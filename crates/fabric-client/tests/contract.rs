//! The Rust half of the client API's contract test: every fixture in
//! `protocol/fixtures` parses into its type and writes back to the same
//! JSON, every op, event kind, refusal reason and variant of every nested
//! enum has a fixture, the messages under `invalid` are refused, and the
//! messages under `tolerated` read as the newer-node rule of ADR 0018
//! says. The TypeScript SDK runs the same checks on the same files.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use fabric_client::api::{
    API_VERSION, Answer, Decision, DoctorReport, Event, Hello, HelloAnswer, JobCreated, JobStatus,
    LedgerEvent, Marker, Op, Outcome, PreflightRow, Process, Reason, Refusal, RefusalKind,
    Registered, Reply, Request, Roots, RunPlan, SessionRecorded, Standing, Step, SystemPath, View,
    Wire, WorkspaceState,
};
use fabric_client::frame;
use fabric_core::harness::HarnessKind;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// What a helper returns: a failure is the test failing, with its reason.
type Fallible<T> = Result<T, Box<dyn Error>>;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/fixtures")
}

/// Every fixture in `dir`, by file name, with its JSON.
fn read_dir(dir: &str) -> Fallible<Vec<(String, Value)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(fixtures().join(dir))? {
        let path = entry?.path();
        let name = path
            .file_name()
            .ok_or("a fixture without a name")?
            .to_string_lossy()
            .into_owned();
        files.push((name, serde_json::from_slice(&fs::read(&path)?)?));
    }
    files.sort_by(|a: &(String, Value), b| a.0.cmp(&b.0));
    if files.is_empty() {
        return Err(format!("no fixtures in {dir}").into());
    }
    Ok(files)
}

/// Parses `value` as a `T` the way a frame from the socket is parsed.
fn at_the_boundary<T: Serialize + DeserializeOwned>(value: &Value) -> Fallible<T> {
    Ok(frame::parse(&serde_json::to_vec(value)?)?)
}

/// Parses every fixture in `dir` as a `T`, checks it writes back to the same JSON, and returns the values.
fn round_trip<T: Serialize + DeserializeOwned>(dir: &str) -> Fallible<Vec<T>> {
    let mut values = Vec::new();
    for (name, json) in read_dir(dir)? {
        let value: T =
            at_the_boundary(&json).map_err(|e| format!("{dir}/{name} does not parse: {e}"))?;
        let written = serde_json::to_value(&value)?;
        if written != json {
            return Err(
                format!("{dir}/{name} does not write back to the same JSON: {written}").into(),
            );
        }
        values.push(value);
    }
    Ok(values)
}

#[test]
fn every_fixture_parses_and_writes_back_to_the_same_json() {
    round_trip::<Hello>("hello").unwrap();
    round_trip::<HelloAnswer>("hello_answer").unwrap();
    round_trip::<Answer>("answer").unwrap();
    round_trip::<Request>("request").unwrap();
    round_trip::<Reply>("reply").unwrap();
    round_trip::<Event>("event").unwrap();
    round_trip::<Refusal>("refusal").unwrap();
}

#[test]
fn the_hello_fixture_pins_the_api_version() {
    let hellos = round_trip::<Hello>("hello").unwrap();
    assert!(hellos.iter().all(|hello| hello.api == API_VERSION));
}

/// The welcome a node sends lists every op of the api version, in order,
/// then every root it applies. A new op or root is additive (ADR 0018,
/// rule 2), and a fixture already on the base never changes within an api
/// version, so the new list gets a welcome fixture of its own: one welcome
/// fixture lists all of it, and every other lists part of it in the same
/// order, as a node of an earlier release of the api did.
#[test]
fn the_welcome_fixtures_list_every_op_and_every_root() {
    let names: Vec<&str> = Op::ALL
        .iter()
        .map(|op| op.name())
        .chain(Roots::capabilities())
        .collect();
    let welcomes: Vec<Vec<String>> = round_trip::<HelloAnswer>("hello_answer")
        .unwrap()
        .into_iter()
        .filter_map(|answer| match answer {
            HelloAnswer::Welcome(welcome) => Some(welcome.capabilities),
            HelloAnswer::Incompatible { .. } => None,
        })
        .collect();
    for listed in &welcomes {
        let mut rest = names.iter();
        assert!(
            listed.iter().all(|name| rest.any(|known| known == name)),
            "a welcome fixture lists {listed:?}, which is not part of {names:?} in its order"
        );
    }
    assert!(
        welcomes.iter().any(|listed| *listed == names),
        "no welcome fixture lists {names:?}"
    );
}

#[test]
fn every_op_has_a_request_and_a_reply_fixture_tagged_with_its_name() {
    let mut requested = BTreeSet::new();
    for request in round_trip::<Request>("request").unwrap() {
        assert_eq!(
            serde_json::to_value(&request).unwrap()["op"],
            request.op().name()
        );
        requested.insert(request.op());
    }
    let mut replied = BTreeSet::new();
    for reply in round_trip::<Reply>("reply").unwrap() {
        assert_eq!(
            serde_json::to_value(&reply).unwrap()["kind"],
            reply.op().name()
        );
        replied.insert(reply.op());
    }
    let all: BTreeSet<Op> = Op::ALL.into_iter().collect();
    assert_eq!(requested, all, "ops without a request fixture");
    assert_eq!(replied, all, "ops without a reply fixture");
}

/// The index of `value`'s variant among the `known` patterns, with how
/// many there are; `None` for the `unknown` one. The match is exhaustive,
/// so a new variant fails to compile until it is listed, and the count
/// comes from the same list, so a test that requires every index then
/// fails until a fixture holds the new variant.
macro_rules! variant {
    ($value:expr, [$($known:pat),+ $(,)?] $(, $unknown:pat)?) => {{
        let known: &[&str] = &[$(stringify!($known)),+];
        let name = match $value {
            $($known => Some(stringify!($known)),)+
            $($unknown => None,)?
        };
        (name.and_then(|name| known.iter().position(|k| *k == name)), known.len())
    }};
}

/// The index of an event's kind among the kinds this client knows, and how many it knows.
fn event_variant(event: &Event) -> (Option<usize>, usize) {
    variant!(
        event,
        [
            Event::CatchingUp,
            Event::Preflight { .. },
            Event::StaleWorkspaceKept { .. },
            Event::AlreadySuspended { .. },
            Event::WorkspaceChanged { .. },
            Event::StoppingHarness { .. },
            Event::FinishingSuspend { .. },
            Event::SessionCaptured { .. },
            Event::SnapshotPublished { .. },
            Event::LeftBehind { .. },
            Event::NoDepartureRecord { .. },
            Event::Suspended { .. },
            Event::Replicated { .. },
            Event::NotReplicated { .. },
            Event::MoveReoffered { .. },
            Event::AlreadyMoved { .. },
            Event::TargetHoldsCopy { .. },
            Event::MoveStarted { .. },
            Event::NoAnswer { .. },
            Event::MoveGaveUp { .. },
            Event::Imported { .. },
            Event::Declined { .. },
            Event::DecisionLeft { .. },
            Event::MoveUndecided { .. },
            Event::MoveUnanswered { .. },
            Event::MoveWaits { .. },
            Event::Fenced { .. },
            Event::CopyMovedAside { .. },
            Event::Pulling { .. },
            Event::HolderStartedMove { .. },
            Event::ImportedHere { .. },
            Event::HolderNotTold { .. },
            Event::WorkspaceInPlace { .. },
            Event::RootChosen { .. },
            Event::CacheCarried { .. },
            Event::CacheRebuilt { .. },
            Event::Restored { .. },
            Event::OrphanSetAside { .. },
            Event::SessionRestored { .. },
            Event::StealWarning { .. },
            Event::Stole { .. },
            Event::OwnerNotTold { .. },
            Event::ContinuingSession { .. },
            Event::NewSession { .. },
            Event::NotCaughtUp { .. },
            Event::PeerSilent { .. },
            Event::LeftAsItWas { .. },
            Event::Frozen { .. },
            Event::WorkspaceGone { .. },
        ],
        Event::Unrecognized
    )
}

#[test]
fn every_event_kind_has_a_fixture() {
    let mut seen = BTreeSet::new();
    let mut kinds = 0;
    for event in round_trip::<Event>("event").unwrap() {
        let (index, count) = event_variant(&event);
        seen.insert(index.expect("a fixture reads as unrecognized"));
        kinds = count;
    }
    assert_eq!(seen, (0..kinds).collect(), "event kinds without a fixture");
}

/// The index of a refusal reason among the reasons this client knows, and how many it knows.
fn reason_variant(reason: &Reason) -> (Option<usize>, usize) {
    variant!(
        reason,
        [
            Reason::UnknownJob { .. },
            Reason::NotOwner { .. },
            Reason::Departed { .. },
            Reason::StaleEpoch { .. },
            Reason::EpochAhead { .. },
            Reason::WrongJob { .. },
            Reason::Departing { .. },
            Reason::Forked { .. },
            Reason::ForkBehind { .. },
            Reason::Unreachable { .. },
            Reason::UnknownPeer { .. },
            Reason::Preflight { .. },
            Reason::StealNeedsForce { .. },
            Reason::HarnessRunning { .. },
            Reason::HarnessGone { .. },
            Reason::HarnessNotRecorded { .. },
            Reason::RanDuringMove { .. },
            Reason::ChangedDuringMove { .. },
            Reason::StaleWorkspace { .. },
            Reason::NoWorkspace { .. },
            Reason::Unfinished { .. },
            Reason::NoSnapshot { .. },
            Reason::WorkspaceOccupied { .. },
            Reason::BadAddress { .. },
            Reason::Peer { .. },
            Reason::Fetch { .. },
            Reason::PeerAhead { .. },
            Reason::PeerBehind { .. },
            Reason::PeerForked { .. },
            Reason::MoveUnlinked { .. },
            Reason::MoveStale { .. },
            Reason::MoveToSelf { .. },
            Reason::StealByOwner { .. },
            Reason::StealWhileDeparting { .. },
            Reason::StealByTarget { .. },
            Reason::StealWithoutSnapshot { .. },
            Reason::SecretInArguments { .. },
            Reason::SyncedRoot { .. },
            Reason::BadRequest,
            Reason::Internal,
        ],
        Reason::Unrecognized
    )
}

#[test]
fn every_refusal_reason_has_a_fixture_with_its_kind() {
    let mut seen = BTreeSet::new();
    let mut reasons = 0;
    for refusal in round_trip::<Refusal>("refusal").unwrap() {
        assert_eq!(refusal.reason().kind(), Some(refusal.kind()));
        let (index, count) = reason_variant(refusal.reason());
        seen.insert(index.expect("a fixture reads as unrecognized"));
        reasons = count;
    }
    assert_eq!(
        seen,
        (0..reasons).collect(),
        "refusal reasons without a fixture"
    );
}

/// Which variants of each enum nested in a message the fixtures hold, by
/// enum, with how many variants each has, and their tags on the wire.
#[derive(Default)]
struct Seen(BTreeMap<&'static str, (BTreeSet<usize>, usize)>, Tags);

/// The tags each enum's variants were seen with, by enum.
type Tags = BTreeMap<&'static str, BTreeSet<String>>;

/// The tag `value` is written with: the string itself, or the `kind` or
/// `type` field of an object. Anything else reads as a tag no pin holds.
fn tag(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        Ok(Value::Object(fields)) => fields
            .get("kind")
            .or_else(|| fields.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("<an object without a tag>")
            .to_owned(),
        Ok(other) => format!("<{other}, not a tagged enum>"),
        Err(e) => format!("<{e}>"),
    }
}

/// The tag `harness` is written with in a message, where the client API's form of it applies.
fn harness_tag(harness: &HarnessKind) -> String {
    let request = SystemPath::parse("/").map(|repo| Request::JobNew {
        repo,
        goal: String::new(),
        harness: harness.clone(),
    });
    match request.map(serde_json::to_value) {
        Ok(Ok(request)) => tag(&request["harness"]),
        Ok(Err(e)) => format!("<{e}>"),
        Err(e) => format!("<{e}>"),
    }
}

impl Seen {
    fn mark(&mut self, name: &'static str, (index, count): (Option<usize>, usize)) {
        let entry = self.0.entry(name).or_default();
        entry.0.insert(index.unwrap_or(usize::MAX));
        entry.1 = count;
    }

    /// As `mark`, also noting the tag the variant is written with.
    fn tagged(&mut self, name: &'static str, tag: String, index: (Option<usize>, usize)) {
        self.mark(name, index);
        self.1.entry(name).or_default().insert(tag);
    }

    fn harness(&mut self, harness: &HarnessKind) {
        let index = variant!(
            harness,
            [
                HarnessKind::ClaudeCode,
                HarnessKind::Codex,
                HarnessKind::Acp { .. },
            ]
        );
        self.tagged("Harness", harness_tag(harness), index);
    }

    fn standing(&mut self, standing: Standing) {
        let index = variant!(
            standing,
            [
                Standing::Owner,
                Standing::Departing,
                Standing::Departed,
                Standing::Replica,
                Standing::Frozen,
                Standing::Unreadable,
            ]
        );
        self.tagged("Standing", tag(&standing), index);
    }

    fn rows(&mut self, rows: &[PreflightRow]) {
        for row in rows {
            let index = variant!(
                row.outcome,
                [
                    Outcome::Equal,
                    Outcome::Fallback,
                    Outcome::Warn,
                    Outcome::Refuse,
                    Outcome::RefuseAlways,
                ]
            );
            self.tagged("Outcome", tag(&row.outcome), index);
        }
    }

    fn refusal(&mut self, refusal: &Refusal) {
        let index = variant!(
            refusal.kind(),
            [
                RefusalKind::NotOwner,
                RefusalKind::Frozen,
                RefusalKind::Unreachable,
                RefusalKind::NeedsForce,
                RefusalKind::JobState,
                RefusalKind::Failed,
            ]
        );
        self.tagged("RefusalKind", tag(&refusal.kind()), index);
        if let Reason::Unfinished { step, .. } = refusal.reason() {
            let index = variant!(step, [Step::Suspend, Step::Restore]);
            self.tagged("Step", tag(step), index);
        }
    }

    fn hello_answer(&mut self, answer: &HelloAnswer) {
        let index = variant!(
            answer,
            [HelloAnswer::Welcome(_), HelloAnswer::Incompatible { .. }]
        );
        self.tagged("HelloAnswer", tag(answer), index);
    }

    fn answer(&mut self, answer: &Answer) {
        let index = variant!(
            answer,
            [
                Answer::Event { .. },
                Answer::Done { .. },
                Answer::Refused { .. },
            ]
        );
        self.tagged("Answer", tag(answer), index);
        match answer {
            Answer::Event { event } => self.event(event),
            Answer::Done { reply } => self.reply(reply),
            Answer::Refused { refusal } => self.refusal(refusal),
        }
    }

    fn request(&mut self, request: &Request) {
        match request {
            Request::JobNew { harness, .. }
            | Request::JobResume {
                harness: Some(harness),
                ..
            }
            | Request::HarnessStarted { harness, .. }
            | Request::HarnessSession { harness, .. } => self.harness(harness),
            Request::JobDecide { decision, .. } => {
                let index = variant!(decision, [Decision::Approve, Decision::Reject]);
                self.tagged("Decision", tag(decision), index);
            }
            _ => {}
        }
    }

    fn reply(&mut self, reply: &Reply) {
        match reply {
            Reply::NodePing(pong) => {
                let index = variant!(pong.wire, [Wire::Quic, Wire::TcpTls]);
                self.tagged("Wire", tag(&pong.wire), index);
            }
            Reply::JobList(list) => {
                for job in &list.jobs {
                    self.standing(job.standing);
                }
            }
            Reply::JobStatus(status) => self.status(status),
            Reply::JobLog(log) => {
                for record in &log.records {
                    self.ledger(&record.event);
                }
            }
            Reply::Doctor { report } => {
                let index = variant!(
                    report,
                    [DoctorReport::Facts { .. }, DoctorReport::Compared { .. }]
                );
                self.tagged("DoctorReport", tag(report), index);
                if let DoctorReport::Compared { rows, .. } = report {
                    self.rows(rows);
                }
            }
            Reply::JobNew(JobCreated { harness, .. })
            | Reply::JobRun(RunPlan { harness, .. })
            | Reply::JobResume(RunPlan { harness, .. })
            | Reply::HarnessStarted(Registered { harness, .. })
            | Reply::HarnessSession(SessionRecorded { harness, .. }) => self.harness(harness),
            _ => {}
        }
    }

    fn status(&mut self, status: &JobStatus) {
        self.harness(&status.harness);
        let index = variant!(
            status.view,
            [
                View::Owner,
                View::Departing { .. },
                View::Departed { .. },
                View::Replica { .. },
                View::Frozen { .. },
                View::Unreadable { .. },
            ]
        );
        self.tagged("View", tag(&status.view), index);
        if let Some(workspace) = &status.workspace {
            let index = variant!(
                workspace.state,
                [
                    WorkspaceState::Current { .. },
                    WorkspaceState::Stale { .. },
                    WorkspaceState::AwaitingDecision { .. },
                    WorkspaceState::CleanCopy,
                    WorkspaceState::Orphan,
                ]
            );
            self.tagged("WorkspaceState", tag(&workspace.state), index);
            if let WorkspaceState::Current { marker } = workspace.state {
                let index = variant!(
                    marker,
                    [
                        Marker::Live,
                        Marker::Suspending,
                        Marker::Clean { .. },
                        Marker::Restoring { .. },
                    ]
                );
                self.tagged("Marker", tag(&marker), index);
            }
        }
        let index = variant!(
            status.process,
            [
                Process::NotRunning,
                Process::Running { .. },
                Process::GroupRemains { .. },
                Process::Unreadable { .. },
            ]
        );
        self.tagged("Process", tag(&status.process), index);
    }

    fn ledger(&mut self, event: &LedgerEvent) {
        let index = variant!(
            event,
            [
                LedgerEvent::Created { .. },
                LedgerEvent::MoveStarted { .. },
                LedgerEvent::MoveImported { .. },
                LedgerEvent::MoveAborted { .. },
                LedgerEvent::Stolen { .. },
                LedgerEvent::SnapshotPublished { .. },
                LedgerEvent::Suspended { .. },
                LedgerEvent::Resumed { .. },
                LedgerEvent::EnvSpecChanged { .. },
                LedgerEvent::DecisionRecorded { .. },
                LedgerEvent::Note { .. },
            ],
            LedgerEvent::Unrecognized
        );
        self.mark("LedgerEvent", index);
        if let LedgerEvent::Created { harness, .. } | LedgerEvent::Resumed { harness, .. } = event {
            self.harness(harness);
        }
    }

    fn event(&mut self, event: &Event) {
        match event {
            Event::Preflight { rows, .. } => self.rows(rows),
            Event::SessionCaptured { harness, .. }
            | Event::SessionRestored { harness, .. }
            | Event::ContinuingSession { harness, .. } => self.harness(harness),
            Event::NewSession { harness, previous } => {
                self.harness(harness);
                if let Some(previous) = previous {
                    self.harness(&previous.harness);
                }
            }
            Event::DecisionLeft { standing } => self.standing(*standing),
            Event::NotReplicated { error, .. }
            | Event::NoAnswer { error, .. }
            | Event::MoveUnanswered { error, .. }
            | Event::HolderNotTold { error, .. }
            | Event::OwnerNotTold { error, .. }
            | Event::NotCaughtUp { error, .. }
            | Event::LeftAsItWas { error } => self.refusal(error),
            _ => {}
        }
    }
}

/// Every nested enum's variants the fixtures hold.
fn seen() -> Fallible<Seen> {
    let mut seen = Seen::default();
    for answer in round_trip::<HelloAnswer>("hello_answer")? {
        seen.hello_answer(&answer);
    }
    for answer in round_trip::<Answer>("answer")? {
        seen.answer(&answer);
    }
    for request in round_trip::<Request>("request")? {
        seen.request(&request);
    }
    for reply in round_trip::<Reply>("reply")? {
        seen.reply(&reply);
    }
    for event in round_trip::<Event>("event")? {
        seen.event(&event);
    }
    for refusal in round_trip::<Refusal>("refusal")? {
        seen.refusal(&refusal);
    }
    Ok(seen)
}

/// The enums whose set of variants is closed: a reader refuses a variant
/// it does not know, so a new one breaks every reader of the api version
/// (ADR 0018, Compatibility rule 3). Ops, event kinds, refusal reasons and
/// ledger record kinds are open and are not here.
const CLOSED: [(&str, &str); 14] = [
    ("Answer", "answer"),
    ("Decision", "decision"),
    ("DoctorReport", "doctor_report"),
    ("Harness", "harness"),
    ("HelloAnswer", "hello_answer"),
    ("Marker", "marker"),
    ("Outcome", "outcome"),
    ("Process", "process"),
    ("RefusalKind", "refusal_kind"),
    ("Standing", "standing"),
    ("Step", "step"),
    ("View", "view"),
    ("Wire", "wire"),
    ("WorkspaceState", "workspace_state"),
];

/// Each closed enum's tags are the ones `protocol/fixtures/closed` pins
/// for this api version. `scripts/check-fixtures.sh` refuses an edit of a
/// pinned file without an api bump, so a new variant of a closed enum,
/// which a fixture of its own would otherwise let through, needs one too
/// (review finding on PR #13).
#[test]
fn every_closed_enum_has_exactly_the_variants_its_api_version_pins() {
    let seen = seen().unwrap();
    let mut pinned = BTreeMap::new();
    for (name, tags) in read_dir("closed").unwrap() {
        let tags: BTreeSet<String> = serde_json::from_value(tags).unwrap();
        pinned.insert(name.trim_end_matches(".json").to_owned(), tags);
    }
    let closed: BTreeMap<String, BTreeSet<String>> = CLOSED
        .iter()
        .map(|(name, file)| ((*file).to_owned(), seen.1[name].clone()))
        .collect();
    assert_eq!(closed, pinned);
}

/// Every key of `value`, at any depth, as a JSON pointer below `at`.
fn pointers(value: &Value, at: &str, into: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                let here = format!("{at}/{}", key.replace('~', "~0").replace('/', "~1"));
                into.push(here.clone());
                pointers(field, &here, into);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                pointers(item, &format!("{at}/{i}"), into);
            }
        }
        _ => {}
    }
}

/// `value` without the key `pointer` names.
fn without(value: &Value, pointer: &str) -> Fallible<Value> {
    let mut value = value.clone();
    let (parent, key) = pointer.rsplit_once('/').ok_or("not a JSON pointer")?;
    let key = key.replace("~1", "/").replace("~0", "~");
    value
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .ok_or("the pointer's parent is not an object")?
        .remove(&key);
    Ok(value)
}

/// The directories of fixtures that are messages of the api.
const MESSAGES: [&str; 7] = [
    "hello",
    "hello_answer",
    "answer",
    "request",
    "reply",
    "event",
    "refusal",
];

/// The keys each message fixture cannot do without: `<dir>/<file> <pointer>`
/// for each key whose removal makes the fixture refused, sorted.
fn required() -> Fallible<Vec<String>> {
    let mut lines = Vec::new();
    for dir in MESSAGES {
        for (name, json) in read_dir(dir)? {
            let mut keys = Vec::new();
            pointers(&json, "", &mut keys);
            for key in keys {
                if parse_as(dir, &without(&json, &key)?).is_err() {
                    lines.push(format!("{dir}/{name} {key}"));
                }
            }
        }
    }
    lines.sort();
    Ok(lines)
}

/// The fields a reader needs are the ones `protocol/required.txt` pins: a
/// field made optional passes every round trip, yet an older reader that
/// needs it breaks once a node leaves it out, and one made required breaks
/// an older writer that leaves it out. `scripts/check-fixtures.sh` refuses
/// a line of the file removed, or added for a fixture the base had,
/// without an api bump (review finding on PR #13). After adding a fixture,
/// `FABRIC_PIN_REQUIRED=1` writes its lines.
#[test]
fn every_message_needs_exactly_the_fields_its_api_version_pins() {
    let path = fixtures().join("../required.txt");
    let required = required().unwrap();
    if std::env::var_os("FABRIC_PIN_REQUIRED").is_some() {
        fs::write(&path, required.join("\n") + "\n").unwrap();
    }
    let pinned = fs::read_to_string(&path).unwrap();
    let pinned: BTreeSet<&str> = pinned.lines().collect();
    let required: BTreeSet<&str> = required.iter().map(String::as_str).collect();
    let unpinned: Vec<&&str> = required.difference(&pinned).collect();
    let optional: Vec<&&str> = pinned.difference(&required).collect();
    assert!(
        unpinned.is_empty() && optional.is_empty(),
        "required but not pinned: {unpinned:#?}\npinned but not required: {optional:#?}"
    );
}

/// ADR 0018 says the two clients cannot drift apart without a test
/// failing. Ops, event kinds and refusal reasons each have a test above;
/// this one holds every other enum a message carries, ledger records
/// included, to a fixture for each variant, so a variant only Rust knows
/// fails here before it fails a TypeScript reader at run time (review
/// finding on PR #13).
#[test]
fn every_variant_of_every_nested_enum_has_a_fixture() {
    let seen = seen().unwrap();
    let names: Vec<&str> = seen.0.keys().copied().collect();
    assert_eq!(
        names,
        [
            "Answer",
            "Decision",
            "DoctorReport",
            "Harness",
            "HelloAnswer",
            "LedgerEvent",
            "Marker",
            "Outcome",
            "Process",
            "RefusalKind",
            "Standing",
            "Step",
            "View",
            "Wire",
            "WorkspaceState",
        ]
    );
    for (name, (indices, count)) in &seen.0 {
        assert_eq!(
            indices,
            &(0..*count).collect::<BTreeSet<_>>(),
            "variants of {name} without a fixture, or a fixture read as unrecognized"
        );
    }
}

/// Parses `value` as the type its fixture names, and writes it back.
fn parse_as(schema: &str, value: &Value) -> Fallible<Value> {
    fn again<T: Serialize + DeserializeOwned>(value: &Value) -> Fallible<Value> {
        Ok(serde_json::to_value(at_the_boundary::<T>(value)?)?)
    }
    match schema {
        "hello" => again::<Hello>(value),
        "hello_answer" => again::<HelloAnswer>(value),
        "request" => again::<Request>(value),
        "reply" => again::<Reply>(value),
        "event" => again::<Event>(value),
        "refusal" => again::<Refusal>(value),
        "answer" => again::<Answer>(value),
        other => Err(format!("no schema named {other}").into()),
    }
}

#[test]
fn every_invalid_message_is_refused_at_the_boundary() {
    for (name, fixture) in read_dir("invalid").unwrap() {
        let schema = fixture["schema"].as_str().unwrap();
        let parsed = parse_as(schema, &fixture["value"]);
        assert!(parsed.is_err(), "invalid/{name} parsed as {parsed:?}");
    }
}

#[test]
fn a_message_from_a_newer_node_reads_as_the_compatibility_rule_says() {
    for (name, fixture) in read_dir("tolerated").unwrap() {
        let schema = fixture["schema"].as_str().unwrap();
        let parsed = parse_as(schema, &fixture["value"])
            .unwrap_or_else(|e| panic!("tolerated/{name} does not parse: {e}"));
        assert_eq!(parsed, fixture["reads_as"], "tolerated/{name}");
    }
}

#[tokio::test]
async fn a_hello_frames_to_the_fixture_bytes_and_back() {
    let fixture: Value =
        serde_json::from_slice(&fs::read(fixtures().join("frame/hello.json")).unwrap()).unwrap();
    let hello: Hello = serde_json::from_value(fixture["value"].clone()).unwrap();
    let hex = frame::encode(&hello)
        .unwrap()
        .iter()
        .fold(String::new(), |mut hex, b| {
            write!(hex, "{b:02x}").unwrap();
            hex
        });
    assert_eq!(hex, fixture["hex"].as_str().unwrap());
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let mut reader: &[u8] = &bytes;
    let read: Hello = frame::read(&mut reader).await.unwrap().unwrap();
    assert_eq!(read, hello);
}
