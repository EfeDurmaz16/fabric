//! The ledger record frame: round trip for arbitrary events, every cut of
//! the tail is a torn tail, and every flipped byte before the tail is
//! corruption with the right offset.

use fabric_core::harness::HarnessKind;
use fabric_core::id::{EnvSpecId, Epoch, EventId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::ledger::{Event, EventKind, Record, RecordError, scan};
use proptest::prelude::*;

fn node() -> impl Strategy<Value = NodeId> {
    any::<[u8; 32]>().prop_map(NodeId::from_bytes)
}

fn kind() -> impl Strategy<Value = EventKind> {
    prop_oneof![
        (".{0,20}", node()).prop_map(|(goal, created_by)| EventKind::Created {
            goal,
            harness: HarnessKind::ClaudeCode,
            created_by
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
            .prop_map(|(n, s, cmd)| EventKind::Resumed {
                node: n,
                snapshot: s.map(SnapshotId::from_bytes),
                harness: HarnessKind::Acp { command: cmd }
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
        match scan(&file) {
            Err(RecordError::Corrupt { offset }) => prop_assert_eq!(offset, 0),
            Err(RecordError::UnknownVersion { .. }) => prop_assert_eq!(i, 4),
            Ok(result) => {
                // A flipped length byte can only make the first frame swallow the
                // second or run past the end; either way the first record is gone.
                prop_assert!(i < 4, "a flipped byte at {i} was accepted");
                prop_assert!(result.records.is_empty());
            }
            Err(e) => return Err(TestCaseError::fail(format!("unexpected {e}"))),
        }
    }
}
