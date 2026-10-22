//! The ledger record frame: round trip for arbitrary events, every cut of
//! the tail is a torn tail, every flipped byte before the tail is corruption
//! with the right offset, and the named tail shapes of ADR 0002.

use std::error::Error;
use std::num::TryFromIntError;

use fabric_core::harness::{HarnessKind, ProgramName};
use fabric_core::id::{EnvSpecId, Epoch, EventId, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::LeaseState;
use fabric_core::ledger::{
    CHECKSUM_LEN, Event, EventKind, HEADER_CHECK_LEN, JobManifest, MAX_RECORD_LEN, Record,
    RecordError, scan,
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
            "[a-z]{1,8}".prop_filter_map("valid program name", |s| ProgramName::parse(&s).ok())
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

/// Returns `varint` spelled one byte longer; postcard reads both as one number.
fn overlong(varint: &[u8]) -> Vec<u8> {
    let mut out = varint.to_vec();
    if let Some(last) = out.last_mut() {
        *last |= 0x80;
    }
    out.push(0);
    out
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

/// The write stopped after the body and before the checksum: the frame has
/// its full length, the body is intact, the checksum bytes are zero. The
/// record was never complete, so it was never acknowledged; dropping it loses
/// nothing.
#[test]
fn a_last_record_whose_checksum_never_landed_is_a_torn_tail() {
    let a = note(None, "a").unwrap();
    let b = note(Some(a.hash()), "b").unwrap();
    let mut file = a.bytes().to_vec();
    file.extend_from_slice(&b.bytes()[..b.bytes().len() - CHECKSUM_LEN]);
    file.resize(a.bytes().len() + b.bytes().len(), 0);
    let result = scan(&file).unwrap();
    assert_eq!(result.records, vec![a.clone()]);
    assert_eq!(result.torn_tail, Some(a.bytes().len()));
}

/// Bit rot, not a torn write: the last record is whole, its checksum bytes
/// are present, and one body bit is flipped. It was complete once, so it may
/// have been acknowledged to a peer; the file fails closed instead of losing
/// it to a truncation.
#[test]
fn a_last_record_with_a_flipped_body_bit_is_corrupt() {
    let a = note(None, "a").unwrap();
    let b = note(Some(a.hash()), "b").unwrap();
    let mut file = a.bytes().to_vec();
    file.extend_from_slice(b.bytes());
    file[a.bytes().len() + HEADER_LEN + 12] ^= 1;
    assert!(matches!(
        scan(&file),
        Err(RecordError::Corrupt { offset }) if offset == a.bytes().len()
    ));
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

    /// The epoch varint spelled one byte longer decodes to the same event
    /// under a checksum that verifies; it would be a second frame hash for one
    /// event, so it is refused as corruption.
    #[test]
    fn an_overlong_varint_inside_a_frame_is_corruption(e in event()) {
        let record = Record::encode(e.clone()).unwrap();
        let body_end = record.bytes().len() - CHECKSUM_LEN;
        let body = &record.bytes()[HEADER_LEN..body_end];
        // version | id | prev tag [| prev hash] | epoch | at | kind
        let epoch_at = 1 + 16 + if e.prev.is_some() { 33 } else { 1 };
        let epoch = postcard::to_stdvec(&e.epoch).unwrap();
        let mut patched = body[..epoch_at].to_vec();
        patched.extend(overlong(&epoch));
        patched.extend_from_slice(&body[epoch_at + epoch.len()..]);
        prop_assert!(
            matches!(
                Record::decode(&frame(&patched).unwrap()),
                Err(RecordError::Corrupt { offset: 0 })
            ),
            "an overlong epoch varint was not reported as corruption at offset 0"
        );
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

/// One event of every kind with fixed ids, encoded in declaration order.
fn every_kind() -> Result<Vec<Event>, Box<dyn Error>> {
    let n = |b: u8| NodeId::from_bytes([b; 32]);
    let s = |b: u8| SnapshotId::from_bytes([b; 32]);
    let m = MoveId::from_bytes([5; 16]);
    let kinds = vec![
        EventKind::Created {
            job_id: JobId::from_bytes([1; 16]),
            goal: "goal".into(),
            harness: HarnessKind::ClaudeCode,
            created_by: n(1),
        },
        EventKind::MoveStarted {
            move_id: m,
            target: n(2),
        },
        EventKind::MoveImported {
            move_id: m,
            from: n(1),
        },
        EventKind::MoveAborted { move_id: m },
        EventKind::Stolen {
            by: n(3),
            from: n(2),
            reason: "asleep".into(),
        },
        EventKind::SnapshotPublished {
            snapshot: s(7),
            parent: Some(s(6)),
        },
        EventKind::Suspended { snapshot: s(7) },
        EventKind::Resumed {
            node: n(3),
            snapshot: Some(s(7)),
            harness: HarnessKind::Acp {
                program: ProgramName::parse("opencode")?,
                args: vec!["acp".into()],
            },
        },
        EventKind::EnvSpecChanged {
            env_spec: EnvSpecId::from_bytes([8; 32]),
        },
        EventKind::DecisionRecorded {
            text: "keep".into(),
        },
        EventKind::Note {
            text: "note".into(),
        },
    ];
    let epoch = Epoch::new(3).ok_or("epoch 3 exists")?;
    kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| {
            Ok(Event {
                id: EventId::from_bytes([u8::try_from(i)?; 16]),
                prev: Some(RecordHash::from_bytes([2; 32])),
                epoch,
                at: 4,
                kind,
            })
        })
        .collect()
}

/// Pins record format version 1: the frame layout and the postcard
/// discriminants and field order of every `EventKind`, one hash per kind so
/// a failure names the kind that moved. Reordering a variant or a field
/// changes a hash and must come with `RECORD_FORMAT_VERSION` bumped once
/// version 1 has shipped.
#[test]
fn record_format_version_one_bytes_are_pinned() {
    let pinned = [
        (
            "Created",
            "3a2557086107ddbc99fecc40e7cf23c7fde2ede23c3772fe884a7eb61afaae4c",
        ),
        (
            "MoveStarted",
            "fbe136b96f3e591c5fc3ac6e2c9e831154a99a9ffd121840fca6abe67718ad76",
        ),
        (
            "MoveImported",
            "a5f7df1fc5a83f9c8f7f2fbcd699e6a2dcf28927bb279b96c65ba9f0c5758558",
        ),
        (
            "MoveAborted",
            "24c568a9f5fe35d5868628095db4ea62cf65afe18e74c1575b45851a8bf7ea9b",
        ),
        (
            "Stolen",
            "b8c84ba7a73d01b5578627bd7f1d0e440f8a9d689d423b0ebd7056f0aea6de70",
        ),
        (
            "SnapshotPublished",
            "42a4dc4add36de0a3e4fe8a43a6203f9645f2fff64ce0037204a5370873c6f70",
        ),
        (
            "Suspended",
            "8108ea18b103b0c4467abe819f1bf676b66ffff7eddbde35260a53bba8ccd0bf",
        ),
        (
            "Resumed",
            "b5a53a32accf869d07ae4c05a9b602c38fcc5bf53172c97cbd6e49f528b60622",
        ),
        (
            "EnvSpecChanged",
            "82808b2959235ee829e7010eb8d6f57da68721e8f1ad82f1bf884463af5e67ff",
        ),
        (
            "DecisionRecorded",
            "1762dbe004656c1b93e3056ab719b345bef215793499335ce742d6b04892c9fc",
        ),
        (
            "Note",
            "4fba635d3b9a5835f5620500e90565e78fd7d558ee386928f0320ba359d8e083",
        ),
    ];
    let events = every_kind().unwrap();
    assert_eq!(events.len(), pinned.len(), "one pin per event kind");
    for (event, (name, hash)) in events.into_iter().zip(pinned) {
        assert_eq!(event.kind.name(), name);
        let record = Record::encode(event).unwrap();
        assert_eq!(
            blake3::hash(record.bytes()).to_hex().as_str(),
            hash,
            "the bytes of {name} moved: {:02x?}",
            record.bytes()
        );
    }
}

/// Pins job manifest format version 1 the same way; a failure prints the bytes.
#[test]
fn job_manifest_version_one_bytes_are_pinned() {
    let manifest = JobManifest {
        job_id: JobId::from_bytes([1; 16]),
        goal: "goal".into(),
        harness: HarnessKind::Codex,
        env_spec: Some(EnvSpecId::from_bytes([8; 32])),
        latest_snapshot: Some(SnapshotId::from_bytes([7; 32])),
        lease: LeaseState::Departing {
            node: NodeId::from_bytes([1; 32]),
            epoch: Epoch::new(3).unwrap(),
            move_id: MoveId::from_bytes([5; 16]),
            target: NodeId::from_bytes([2; 32]),
        },
        head: RecordHash::from_bytes([2; 32]),
        event_count: 9,
        created_at: 4,
        updated_at: 5,
    };
    let bytes = manifest.encode().unwrap();
    assert_eq!(
        blake3::hash(&bytes).to_hex().as_str(),
        "c6a0d10da670e5658a9aee5bc78703d6e0e94ed26e4d6b07c099f1b861986c85",
        "the job manifest bytes moved: {bytes:02x?}"
    );
    assert_eq!(JobManifest::decode(&bytes).unwrap(), manifest);
}
