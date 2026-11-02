//! The journal is never written on the async runtime: every call the
//! bridge client makes on its log runs on the log's own thread, and the
//! number of calls, thread hops and committed writes per step is pinned.
//! These are counts, not times: they hold at any machine load.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::sync::{Arc, Mutex};
use std::task::{Context, Wake, Waker};
use std::time::Duration;

use common::{create, double, journal_for, operation, scope};
use fabric_bridge::wire::{Action, ExecParams, Locator};
use fabric_bridge::{
    Answer, Bridge, BridgeError, Config, Effect, Entry, LogError, LogThread, MemoryLog,
    OperationLog,
};
use fabric_core::managed::OperationId;
use fabric_journal::{Journal, ProviderAnswer, ProviderDispatch, ProviderStatus};

/// What a [`Watched`] log saw: each call's kind and the thread it ran on.
type Seen = Arc<Mutex<Vec<(&'static str, String)>>>;

/// The journal, noting the thread each call runs on.
struct Watched {
    journal: Journal,
    seen: Seen,
}

impl Watched {
    fn note(&self, call: &'static str) {
        let thread = std::thread::current().name().unwrap_or("").to_owned();
        self.seen.lock().unwrap().push((call, thread));
    }
}

impl OperationLog for Watched {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        self.note("begin");
        self.journal.begin(dispatch, refusal)
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError> {
        self.note("record");
        self.journal.record(operation, status, answer, locator)
    }

    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError> {
        self.note("locate");
        self.journal.locate(operation, locator)
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        self.note("entry");
        self.journal.entry(operation)
    }
}

fn config() -> Config {
    let mut config = Config::new("fixture");
    config.grace = Duration::from_millis(200);
    config.kill_grace = Duration::from_millis(500);
    config.handshake_timeout = Duration::from_secs(5);
    config
}

fn fx(key: &str) -> Locator {
    Locator::from_value(&serde_json::json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": "proj-1"},
        "kind": "sandbox",
        "native_id": format!("fx-{key}"),
    }))
    .unwrap()
}

fn exec(n: u8, target: &Locator, timeout_ms: u64) -> Effect {
    Effect {
        operation: operation(n),
        scope: scope(),
        action: Action::Exec {
            target: target.clone(),
            params: ExecParams {
                argv: vec!["true".into()],
                stdin: None,
                cwd: None,
                timeout_ms,
                max_output_bytes: 1024,
                secret_names: Vec::new(),
            },
        },
        approval: None,
        policy: None,
        deadline_ms: 30_000,
    }
}

/// One step's counts: calls run on the log thread, and the log calls
/// they made that write (each one committed transaction, so one fsync of
/// the journal's write-ahead log) and that only read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    calls: u64,
    writes: usize,
    reads: usize,
}

/// The counts since `(calls, seen)` were taken.
fn since(log: &LogThread<Watched>, seen: &Seen, from: (u64, usize)) -> Counts {
    let seen = seen.lock().unwrap();
    let new = &seen[from.1..];
    Counts {
        calls: log.calls() - from.0,
        writes: new.iter().filter(|(call, _)| *call != "entry").count(),
        reads: new.iter().filter(|(call, _)| *call == "entry").count(),
    }
}

/// Every log call of a create, an exec, a create with sixteen `progress`
/// frames, a refusal before sending, a timeout and an observe runs on the
/// log thread, never on the runtime's; each step makes the calls the
/// table pins, so a step that writes more, or hops more, fails here.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one log walked through six steps, then the table"
)]
async fn every_log_call_runs_on_the_log_thread_and_each_step_makes_its_pinned_calls() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Seen::default();
    let mut log = LogThread::spawn(Watched {
        journal: journal_for(dir.path(), scope()),
        seen: Arc::clone(&seen),
    })
    .unwrap();
    let bridge = Bridge::spawn(double(), config()).await.unwrap();
    let mut table = Vec::new();
    // Each step: record where the counts were, run it, take the counts.
    let from = (log.calls(), seen.lock().unwrap().len());
    let created = bridge
        .request(&mut log, create(1, "fixture/ok"))
        .await
        .unwrap()
        .finish(&mut log)
        .await
        .unwrap();
    table.push(("create", created.status, since(&log, &seen, from)));

    let from = (log.calls(), seen.lock().unwrap().len());
    let ran = bridge
        .request(&mut log, exec(2, &fx("key-1"), 1000))
        .await
        .unwrap()
        .finish(&mut log)
        .await
        .unwrap();
    table.push(("exec", ran.status, since(&log, &seen, from)));

    let from = (log.calls(), seen.lock().unwrap().len());
    let progressed = bridge
        .request(&mut log, create(3, "double/progress-16"))
        .await
        .unwrap()
        .finish(&mut log)
        .await
        .unwrap();
    table.push((
        "create, 16 progress",
        progressed.status,
        since(&log, &seen, from),
    ));

    let from = (log.calls(), seen.lock().unwrap().len());
    let refused = bridge.request(&mut log, exec(4, &fx("key-1"), 0)).await;
    assert!(
        matches!(refused, Err(BridgeError::RefusedLocally(_))),
        "{refused:?}"
    );
    table.push((
        "refused before sending",
        ProviderStatus::Rejected,
        since(&log, &seen, from),
    ));

    let from = (log.calls(), seen.lock().unwrap().len());
    let mut silent = create(5, "double/silent");
    silent.deadline_ms = 1;
    let timed_out = bridge
        .request(&mut log, silent)
        .await
        .unwrap()
        .finish(&mut log)
        .await
        .unwrap();
    assert_eq!(timed_out.answer, Answer::TimedOut);
    table.push(("timed out", timed_out.status, since(&log, &seen, from)));

    let from = (log.calls(), seen.lock().unwrap().len());
    let observed = bridge
        .observe(
            &mut log,
            OperationId::from_bytes([3; 16]),
            "sandbox.create",
            fx("key-3"),
            1000,
        )
        .await;
    assert!(
        matches!(observed, Err(BridgeError::NotObservable(_))),
        "a succeeded create has nothing to observe: {observed:?}"
    );
    table.push((
        "observe refused",
        ProviderStatus::Succeeded,
        since(&log, &seen, from),
    ));

    eprintln!("step | status | log thread calls | thread hops | writes (fsyncs) | reads");
    for (name, status, c) in &table {
        eprintln!(
            "{name} | {} | {} | {} | {} | {}",
            status.name(),
            c.calls,
            2 * c.calls,
            c.writes,
            c.reads
        );
    }
    let counts = |calls, writes, reads| Counts {
        calls,
        writes,
        reads,
    };
    let want = [
        ("create", ProviderStatus::Succeeded, counts(2, 2, 1)),
        ("exec", ProviderStatus::Succeeded, counts(2, 2, 1)),
        (
            "create, 16 progress",
            ProviderStatus::Succeeded,
            counts(18, 18, 1),
        ),
        (
            "refused before sending",
            ProviderStatus::Rejected,
            counts(1, 1, 0),
        ),
        ("timed out", ProviderStatus::Unknown, counts(2, 2, 1)),
        (
            "observe refused",
            ProviderStatus::Succeeded,
            counts(1, 0, 1),
        ),
    ];
    assert_eq!(table, want);

    let threads: Vec<(&str, String)> = seen.lock().unwrap().clone();
    assert!(!threads.is_empty());
    for (call, thread) in &threads {
        assert_eq!(thread, "fabric-bridge-log", "{call} ran on {thread:?}");
    }
    let watched = log.into_inner().await.unwrap();
    let row = watched
        .journal
        .provider_operation(OperationId::from_bytes([1; 16]))
        .unwrap()
        .unwrap();
    assert_eq!(row.status, ProviderStatus::Succeeded);
    assert!(bridge.close().await.is_some());
}

/// A waker that reads the log thread's count of calls when it is woken,
/// which a log thread does as it hands a call's result over.
struct CountOnWake {
    log: Arc<LogThread<MemoryLog>>,
    heard: Mutex<std::sync::mpsc::Sender<u64>>,
}

impl Wake for CountOnWake {
    fn wake(self: Arc<Self>) {
        let _ = self.heard.lock().unwrap().send(self.log.calls());
    }
}

/// A call is counted before its caller can hear its result, so a count
/// read once a call returned takes that call in, as the table above
/// needs. The second call's waker reads the count at the moment its
/// result is handed over; the first call holds the log thread until that
/// waker is in place, so the order is the same on every run.
#[test]
fn a_call_is_counted_before_its_caller_hears_its_result() {
    let log = Arc::new(LogThread::spawn(MemoryLog::new()).unwrap());
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let mut held = Box::pin(log.call(move |_| gate.recv().unwrap()));
    let mut idle = Context::from_waker(Waker::noop());
    assert!(held.as_mut().poll(&mut idle).is_pending());
    let (heard, count) = std::sync::mpsc::channel();
    let waker = Waker::from(Arc::new(CountOnWake {
        log: Arc::clone(&log),
        heard: Mutex::new(heard),
    }));
    let mut second = Box::pin(log.call(|_| ()));
    assert!(
        second
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    open.send(()).unwrap();
    assert_eq!(count.recv().unwrap(), 2);
}

/// The journal, with a `begin` that takes `delay` first, as a slow disk
/// would.
struct Slow {
    journal: Journal,
    delay: Duration,
}

impl OperationLog for Slow {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        std::thread::sleep(self.delay);
        self.journal.begin(dispatch, refusal)
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError> {
        self.journal.record(operation, status, answer, locator)
    }

    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError> {
        self.journal.locate(operation, locator)
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        self.journal.entry(operation)
    }
}

/// Recording an operation and queueing its frame are one call on the log
/// thread: a caller that stops waiting while the journal commits leaves
/// the operation recorded and its frame sent whole. The worker answers it,
/// late, and the locator it names is recorded; the status stays
/// `dispatched`, as for any request whose caller dropped it.
#[tokio::test]
async fn a_caller_that_stops_waiting_on_the_log_leaves_the_operation_recorded_and_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = LogThread::spawn(Slow {
        journal: journal_for(dir.path(), scope()),
        delay: Duration::from_millis(300),
    })
    .unwrap();
    let bridge = Bridge::spawn(double(), config()).await.unwrap();
    let gave_up = tokio::time::timeout(
        Duration::from_millis(50),
        bridge.request(&mut log, create(1, "fixture/ok")),
    )
    .await;
    assert!(
        gave_up.is_err(),
        "the log answered before the caller gave up"
    );
    let started = std::time::Instant::now();
    while bridge.late_answers() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the frame was never sent"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bridge.record_late(&mut log).await.unwrap();
    let row = log
        .call(|log| {
            log.journal
                .provider_operation(OperationId::from_bytes([1; 16]))
                .unwrap()
                .unwrap()
        })
        .await
        .unwrap();
    assert_eq!(row.status, ProviderStatus::Dispatched);
    assert_eq!(row.locator, Some(fx("key-1").to_value().to_string()));
    // Nothing of the dropped request holds the worker's in-flight room.
    let next = bridge
        .request(&mut log, create(2, "fixture/ok"))
        .await
        .unwrap()
        .finish(&mut log)
        .await
        .unwrap();
    assert_eq!(next.status, ProviderStatus::Succeeded);
    assert!(bridge.close().await.is_some());
}
