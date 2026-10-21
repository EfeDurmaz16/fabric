//! The Rust half of the client API's contract test: every fixture in
//! `protocol/fixtures` parses into its type and writes back to the same
//! JSON, every op, event kind and refusal reason has a fixture, the
//! messages under `invalid` are refused, and the messages under
//! `tolerated` read as the newer-node rule of ADR 0018 says. The
//! TypeScript SDK runs the same checks on the same files.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use fabric_client::api::{
    API_VERSION, Answer, Event, Hello, HelloAnswer, Op, Reason, Refusal, Reply, Request,
};
use fabric_client::frame;
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

/// Parses every fixture in `dir` as a `T`, checks it writes back to the same JSON, and returns the values.
fn round_trip<T: Serialize + DeserializeOwned>(dir: &str) -> Fallible<Vec<T>> {
    let mut values = Vec::new();
    for (name, json) in read_dir(dir)? {
        let value: T = serde_json::from_value(json.clone())
            .map_err(|e| format!("{dir}/{name} does not parse: {e}"))?;
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

/// The welcome a node sends lists every op of the api version, in order.
#[test]
fn the_welcome_fixture_lists_every_op() {
    let welcome = round_trip::<HelloAnswer>("hello_answer")
        .unwrap()
        .into_iter()
        .find_map(|answer| match answer {
            HelloAnswer::Welcome(welcome) => Some(welcome),
            HelloAnswer::Incompatible { .. } => None,
        })
        .unwrap();
    let names: Vec<&str> = Op::ALL.iter().map(|op| op.name()).collect();
    assert_eq!(welcome.capabilities, names);
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

/// The index of an event's kind; adding a kind fails to compile here until
/// it has an index, and the test below fails until it has a fixture.
fn event_index(event: &Event) -> usize {
    match event {
        Event::CatchingUp => 0,
        Event::Preflight { .. } => 1,
        Event::StaleWorkspaceKept { .. } => 2,
        Event::AlreadySuspended { .. } => 3,
        Event::WorkspaceChanged { .. } => 4,
        Event::StoppingHarness { .. } => 5,
        Event::FinishingSuspend { .. } => 6,
        Event::SessionCaptured { .. } => 7,
        Event::SnapshotPublished { .. } => 8,
        Event::LeftBehind { .. } => 9,
        Event::NoDepartureRecord { .. } => 10,
        Event::Suspended { .. } => 11,
        Event::Replicated { .. } => 12,
        Event::NotReplicated { .. } => 13,
        Event::MoveReoffered { .. } => 14,
        Event::AlreadyMoved { .. } => 15,
        Event::TargetHoldsCopy { .. } => 16,
        Event::MoveStarted { .. } => 17,
        Event::NoAnswer { .. } => 18,
        Event::MoveGaveUp { .. } => 19,
        Event::Imported { .. } => 20,
        Event::Declined { .. } => 21,
        Event::DecisionLeft { .. } => 22,
        Event::MoveUndecided { .. } => 23,
        Event::MoveUnanswered { .. } => 24,
        Event::MoveWaits { .. } => 25,
        Event::Fenced { .. } => 26,
        Event::CopyMovedAside { .. } => 27,
        Event::Pulling { .. } => 28,
        Event::HolderStartedMove { .. } => 29,
        Event::ImportedHere { .. } => 30,
        Event::HolderNotTold { .. } => 31,
        Event::WorkspaceInPlace { .. } => 32,
        Event::RootChosen { .. } => 33,
        Event::CacheCarried { .. } => 34,
        Event::CacheRebuilt { .. } => 35,
        Event::Restored { .. } => 36,
        Event::OrphanSetAside { .. } => 37,
        Event::SessionRestored { .. } => 38,
        Event::StealWarning { .. } => 39,
        Event::Stole { .. } => 40,
        Event::OwnerNotTold { .. } => 41,
        Event::ContinuingSession { .. } => 42,
        Event::NewSession { .. } => 43,
        Event::NotCaughtUp { .. } => 44,
        Event::PeerSilent { .. } => 45,
        Event::LeftAsItWas { .. } => 46,
        Event::Frozen { .. } => 47,
        Event::WorkspaceGone { .. } => 48,
        Event::Unrecognized => usize::MAX,
    }
}

#[test]
fn every_event_kind_has_a_fixture() {
    let seen: BTreeSet<usize> = round_trip::<Event>("event")
        .unwrap()
        .iter()
        .map(event_index)
        .collect();
    assert!(
        !seen.contains(&usize::MAX),
        "a fixture reads as unrecognized"
    );
    assert_eq!(seen, (0..49).collect(), "event kinds without a fixture");
}

/// The index of a refusal reason, as `event_index` is for events.
fn reason_index(reason: &Reason) -> usize {
    match reason {
        Reason::UnknownJob { .. } => 0,
        Reason::NotOwner { .. } => 1,
        Reason::Departed { .. } => 2,
        Reason::StaleEpoch { .. } => 3,
        Reason::EpochAhead { .. } => 4,
        Reason::WrongJob { .. } => 5,
        Reason::Departing { .. } => 6,
        Reason::Forked { .. } => 7,
        Reason::ForkBehind { .. } => 8,
        Reason::Unreachable { .. } => 9,
        Reason::UnknownPeer { .. } => 10,
        Reason::Preflight { .. } => 11,
        Reason::StealNeedsForce { .. } => 12,
        Reason::HarnessRunning { .. } => 13,
        Reason::HarnessGone { .. } => 14,
        Reason::HarnessNotRecorded { .. } => 15,
        Reason::RanDuringMove { .. } => 16,
        Reason::ChangedDuringMove { .. } => 17,
        Reason::StaleWorkspace { .. } => 18,
        Reason::NoWorkspace { .. } => 19,
        Reason::Unfinished { .. } => 20,
        Reason::NoSnapshot { .. } => 21,
        Reason::WorkspaceOccupied { .. } => 22,
        Reason::BadAddress { .. } => 23,
        Reason::Peer { .. } => 24,
        Reason::Fetch { .. } => 25,
        Reason::BadRequest => 26,
        Reason::Internal => 27,
        Reason::Unrecognized => usize::MAX,
    }
}

#[test]
fn every_refusal_reason_has_a_fixture_with_its_kind() {
    let refusals = round_trip::<Refusal>("refusal").unwrap();
    for refusal in &refusals {
        assert_eq!(refusal.reason().kind(), Some(refusal.kind()));
    }
    let seen: BTreeSet<usize> = refusals.iter().map(|r| reason_index(r.reason())).collect();
    assert_eq!(seen, (0..28).collect(), "refusal reasons without a fixture");
}

/// Parses `value` as the type its fixture names, and writes it back.
fn parse_as(schema: &str, value: Value) -> Fallible<Value> {
    fn again<T: Serialize + DeserializeOwned>(value: Value) -> Fallible<Value> {
        Ok(serde_json::to_value(serde_json::from_value::<T>(value)?)?)
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
        let parsed = parse_as(schema, fixture["value"].clone());
        assert!(parsed.is_err(), "invalid/{name} parsed as {parsed:?}");
    }
}

#[test]
fn a_message_from_a_newer_node_reads_as_the_compatibility_rule_says() {
    for (name, fixture) in read_dir("tolerated").unwrap() {
        let schema = fixture["schema"].as_str().unwrap();
        let parsed = parse_as(schema, fixture["value"].clone())
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
