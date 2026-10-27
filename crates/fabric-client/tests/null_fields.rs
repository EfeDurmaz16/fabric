//! ADR 0018 says a reader takes `null` for a field that may be left out,
//! as the TypeScript SDK does (M2-11). Ignored until the Rust reader does
//! for every such field.

use fabric_client::api::Event;
use fabric_client::frame::parse;

/// `git_bundle` may be left out of `snapshot_published`, so `null` reads
/// as left out: `false`.
#[test]
#[ignore = "M2-11: expected failure until fixed"]
fn a_null_git_bundle_reads_as_absent() {
    let json = br#"{"kind":"snapshot_published","snapshot":"abababababababababababababababababababababababababababababababab","files":1,"bytes":1,"new_objects":1,"git_bundle":null}"#;
    let event: Event = parse(json).unwrap();
    assert!(matches!(
        event,
        Event::SnapshotPublished {
            git_bundle: false,
            ..
        }
    ));
}
