//! The ledger record frame: round trip for arbitrary events, every cut of
//! the tail is a torn tail, every flipped byte before the tail is corruption
//! with the right offset, and the named tail shapes of ADR 0002.

use std::num::TryFromIntError;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{EnvSpecId, Epoch, EventId, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::ledger::{
    CHECKSUM_LEN, Event, EventKind, HEADER_CHECK_LEN, MAX_RECORD_LEN, Record, RecordError, scan,
};
use proptest::prelude::*;

fn node() -> impl Strategy<Value = NodeId> {
    any::<[u8; 32]>().prop_map(NodeId::from_bytes)
}

fn kind() -> impl Strategy<Value = EventKind> {
    prop_oneof![
        (any::<[u8; 16]>(), ".{0,20}", node()).prop_map(|(job, goal, created_by)| {
            EventKind::Created {
                job_id: JobId::from_bytes(job),
                goal,
                harness: HarnessKind::ClaudeCode,
                created_by,
            }
        }),
        (any::<[u8; 16]>(), node()).prop_map(|(m, target)| EventKind::MoveStarted {
            move_id: MoveId::from_bytes(m),
            target
        }),
        (any::<[u8; 16]>(), node()).prop_map(|(m, from)| EventKind::MoveImported {
            move_id: MoveId::from_bytes(m),
            from
        }),
        any::<[u8; 16]>().prop_map(|m| EventKind::MoveAborted {
            move_id: MoveId::from_bytes(m)
        }),
        (node(), node(), ".{0,20}").prop_map(|(by, from, reason)| EventKind::Stolen {
            by,
            from,
            reason
        }),
        (any::<[u8; 32]>(), proptest::option::of(any::<[u8; 32]>())).prop_map(|(s, p)| {
            EventKind::SnapshotPublished {
                snapshot: SnapshotId::from_bytes(s),
                parent: p.map(SnapshotId::from_bytes),
            }
        }),
        any::<[u8; 32]>().prop_map(|s| EventKind::Suspended {
            snapshot: SnapshotId::from_bytes(s)
        }),
        (
            node(),
            proptest::option::of(any::<[u8; 32]>()),
            "[a-z]{1,8}"
        )
            .prop_map(|(n, s, program)| EventKind::Resumed {
                node: n,
                snapshot: s.map(SnapshotId::from_bytes),
                harness: HarnessKind::Acp {
                    program,
                    args: vec!["acp".into()],
                },
            }),
        any::<[u8; 32]>().prop_map(|e| EventKind::EnvSpecChanged {
            env_spec: EnvSpecId::from_bytes(e)
        }),
        ".{0,40}".prop_map(|text| EventKind::DecisionRecorded { text }),
        ".{0,40}".prop_map(|text| EventKind::Note { text }),
    ]
}

fn event() -> impl Strategy<Value = Event> {
    (
        any::<[u8; 16]>(),
        proptest::option::of(any::<[u8; 32]>()),
        (1u64..1000).prop_filter_map("epoch zero does not exist", Epoch::new),
        any::<u64>(),
        kind(),
    )
        .prop_map(|(id, prev, epoch, at, kind)| Event {
            id: EventId::from_bytes(id),
            prev: prev.map(RecordHash::from_bytes),
            epoch,
            at,
            kind,
        })
}

const HEADER_LEN: usize = 4 + HEADER_CHECK_LEN;

fn note(prev: Option<RecordHash>, text: &str) -> Result<Record, RecordError> {
    Record::encode(Event {
        id: EventId::from_bytes([1; 16]),
        prev,
        epoch: Epoch::FIRST,
        at: 1,
        kind: EventKind::Note { text: text.into() },
    })
}

/// Frames `body` (version and payload) with a header and a checksum the
/// reader accepts, so a test can put any bytes inside a well-formed frame.
fn frame(body: &[u8]) -> Result<Vec<u8>, TryFromIntError> {
    let length = u32::try_from(body.len() + CHECKSUM_LEN)?;
    let mut out = length.to_le_bytes().to_vec();
    out.extend_from_slice(&blake3::hash(&length.to_le_bytes()).as_bytes()[..HEADER_CHECK_LEN]);
    out.extend_from_slice(body);
    let digest = blake3::hash(&out);
    out.extend_from_slice(&digest.as_bytes()[..CHECKSUM_LEN]);
    Ok(out)
}

/// A frame that carries bytes after the event, with a checksum that covers
/// them, is not a second valid spelling of the event; it is corruption.
#[test]
fn bytes_after_the_event_inside_a_frame_are_corruption() {
    let record = note(None, "n").unwrap();
    let body_end = record.bytes().len() - CHECKSUM_LEN;
    let mut body = record.bytes()[HEADER_LEN..body_end].to_vec();
    body.push(0xee);
    assert!(matches!(
        Record::decode(&frame(&body).unwrap()),
        Err(RecordError::Corrupt { offset: 0 })
    ));
}

/// The power-loss shape on APFS and ext4: the file has the record's full
/// length and the header landed, but the pages after it read back as zero.
#[test]
fn a_last_record_with_a_zeroed_body_is_a_torn_tail() {
    let a = note(None, "a").unwrap();
    let b = note(Some(a.hash()), "b").unwrap();
    let mut file = a.bytes().to_vec();
    file.extend_from_slice(&b.bytes()[..HEADER_LEN]);
    file.resize(a.bytes().len() + b.bytes().len(), 0);
    let result = scan(&file).unwrap();
    assert_eq!(result.records, vec![a.clone()]);
    assert_eq!(result.torn_tail, Some(a.bytes().len()));
}

#[test]
fn an_all_zero_tail_is_a_torn_tail() {
    let a = note(None, "a").unwrap();
    let mut file = a.bytes().to_vec();
    file.extend_from_slice(&[0; 9]);
    let result = scan(&file).unwrap();
    assert_eq!(result.records, vec![a.clone()]);
    assert_eq!(result.torn_tail, Some(a.bytes().len()));
}

/// A verified header that declares more than the bound is refused before
/// any allocation, in a file and in a frame from a peer.
#[test]
fn a_declared_length_over_the_bound_is_refused() {
    let length = u32::try_from(MAX_RECORD_LEN - HEADER_LEN + 1).unwrap();
    let mut file = length.to_le_bytes().to_vec();
    file.extend_from_slice(&blake3::hash(&length.to_le_bytes()).as_bytes()[..HEADER_CHECK_LEN]);
    file.push(1);
    assert!(matches!(
        scan(&file),
        Err(RecordError::TooLarge { len }) if len == MAX_RECORD_LEN + 1
    ));
    let big = Record::encode(Event {
        id: EventId::from_bytes([1; 16]),
        prev: None,
        epoch: Epoch::FIRST,
        at: 1,
        kind: EventKind::Note {
            text: "x".repeat(MAX_RECORD_LEN),
        },
    });
    assert!(matches!(big, Err(RecordError::TooLarge { .. })));
}

proptest! {
    #[test]
    fn frame_round_trips(e in event()) {
        let record = Record::encode(e.clone()).unwrap();
        let back = Record::decode(record.bytes()).unwrap();
        prop_assert_eq!(back.event(), &e);
        prop_assert_eq!(back.hash(), record.hash());
        prop_assert_eq!(back.hash(), RecordHash::of(record.bytes()));
    }

    #[test]
    fn any_cut_of_the_last_record_is_a_torn_tail(a in event(), b in event(), cut in any::<proptest::sample::Index>()) {
        let a = Record::encode(a).unwrap();
        let b = Record::encode(b).unwrap();
        let mut file = a.bytes().to_vec();
        file.extend_from_slice(b.bytes());
        // Keep at least one byte of the second record and never all of it.
        let keep = a.bytes().len() + 1 + cut.index(b.bytes().len() - 1);
        let result = scan(&file[..keep]).unwrap();
        prop_assert_eq!(result.records, vec![a.clone()]);
        prop_assert_eq!(result.torn_tail, Some(a.bytes().len()));
    }

    #[test]
    fn any_flipped_byte_before_the_tail_is_corruption(a in event(), b in event(), at in any::<proptest::sample::Index>(), bit in 0u8..8) {
        let a = Record::encode(a).unwrap();
        let b = Record::encode(b).unwrap();
        let mut file = a.bytes().to_vec();
        file.extend_from_slice(b.bytes());
        let i = at.index(a.bytes().len());
        file[i] ^= 1 << bit;
        // The length bytes included: a damaged length fails the header check
        // and can never turn the records after it into a torn tail.
        prop_assert!(
            matches!(scan(&file), Err(RecordError::Corrupt { offset: 0 })),
            "a flipped bit at byte {i} was not reported as corruption at offset 0"
        );
    }
}
