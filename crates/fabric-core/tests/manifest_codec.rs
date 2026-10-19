//! The snapshot manifest codec: round trip for arbitrary manifests and a
//! pinned hash, so a change to the wire format is a failing test, not a
//! surprise on the other machine.

use fabric_core::harness::HarnessState;
use fabric_core::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use fabric_core::path::{AbsPath, PathError, RelPath};
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, Entry, EntryKind, GitHead, GitState, MANIFEST_VERSION,
    ManifestError, Platform, SnapshotManifest,
};
use proptest::prelude::*;

fn component() -> impl Strategy<Value = String> {
    "[a-z0-9_][a-z0-9_.-]{0,7}".prop_filter("no dot components", |s| {
        s != "." && s != ".." && s != ".git"
    })
}

fn rel_path() -> impl Strategy<Value = RelPath> {
    proptest::collection::vec(component(), 1..4).prop_filter_map("valid relative path", |parts| {
        RelPath::parse(&parts.join("/")).ok()
    })
}

fn abs_path() -> impl Strategy<Value = AbsPath> {
    proptest::collection::vec(component(), 0..4).prop_filter_map("valid absolute path", |parts| {
        AbsPath::parse(&format!("/{}", parts.join("/"))).ok()
    })
}

fn chunk() -> impl Strategy<Value = ChunkId> {
    any::<[u8; 32]>().prop_map(ChunkId::from_bytes)
}

fn entry() -> impl Strategy<Value = Entry> {
    (
        rel_path(),
        prop_oneof![
            Just(EntryKind::File),
            Just(EntryKind::Dir),
            "[a-z/.]{1,12}".prop_map(|target| EntryKind::Symlink { target }),
        ],
        0u32..0o777,
        any::<u64>(),
        any::<u64>(),
        proptest::collection::vec(chunk(), 0..3),
    )
        .prop_map(|(rel_path, kind, mode, mtime_ns, size, chunks)| {
            let chunks = if kind == EntryKind::File {
                chunks
            } else {
                Vec::new()
            };
            Entry {
                rel_path,
                kind,
                mode,
                mtime_ns,
                size,
                chunks,
            }
        })
}

fn unique_entries() -> impl Strategy<Value = Vec<Entry>> {
    proptest::collection::vec(entry(), 0..6).prop_map(|mut entries| {
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.rel_path.clone()));
        entries
    })
}

fn carry_key() -> impl Strategy<Value = CarryKey> {
    prop_oneof![
        (
            "[0-9]{1,2}",
            "(npm|pnpm|yarn|bun)",
            "[0-9]{1,2}",
            proptest::option::of("[a-z/]{1,10}")
        )
            .prop_map(|(node_major, package_manager, pm_major, pnpm_store_path)| {
                CarryKey::Node {
                    node_major,
                    package_manager,
                    pm_major,
                    pnpm_store_path,
                }
            }),
        ("rustc [0-9.]{5}", abs_path()).prop_map(|(rustc_vv, cargo_home)| CarryKey::Rust {
            rustc_vv,
            cargo_home
        }),
        ("[0-9.]{3,6}", abs_path()).prop_map(|(python_version, python_home)| CarryKey::Python {
            python_version,
            python_home
        }),
        ("[a-z]{1,6}", "[0-9.]{1,6}").prop_map(|(tool, version)| CarryKey::Other { tool, version }),
    ]
}

fn cache_entry() -> impl Strategy<Value = CacheEntry> {
    (
        rel_path(),
        prop_oneof![
            Just(CacheClass::Deps),
            Just(CacheClass::Build),
            Just(CacheClass::ToolCache)
        ],
        any::<bool>(),
        carry_key(),
        any::<bool>(),
    )
        .prop_map(|(path, class, carried, key, partial)| CacheEntry {
            path,
            class,
            carried,
            key,
            partial,
        })
}

fn git_state() -> impl Strategy<Value = GitState> {
    (
        proptest::collection::vec(chunk(), 1..3),
        proptest::option::of(any::<[u8; 32]>().prop_map(SnapshotId::from_bytes)),
        proptest::collection::vec(("refs/[a-z/]{1,10}", "[0-9a-f]{40}"), 0..3),
        prop_oneof![
            "[a-z-]{1,8}".prop_map(GitHead::Branch),
            "[0-9a-f]{40}".prop_map(GitHead::Detached)
        ],
        "[0-9a-f]{40}",
        "[0-9a-f]{40}",
        proptest::collection::vec((rel_path(), any::<u64>()), 0..3),
        proptest::collection::vec(("[a-z.]{1,10}", "[a-z:/.]{0,10}"), 0..3),
        proptest::option::of(proptest::collection::vec(chunk(), 1..2)),
    )
        .prop_map(
            |(
                bundle,
                bundle_base,
                refs,
                head,
                index_commit,
                worktree_commit,
                worktree_mtimes,
                config,
                exclude,
            )| GitState {
                bundle,
                bundle_base,
                refs,
                head,
                index_commit,
                worktree_commit,
                worktree_mtimes,
                config,
                exclude,
            },
        )
}

fn manifest() -> impl Strategy<Value = SnapshotManifest> {
    (
        any::<[u8; 16]>(),
        abs_path(),
        proptest::option::of(any::<[u8; 32]>()),
        any::<u64>(),
        any::<[u8; 32]>(),
        ("(macos|linux)", "(arm64|x86_64)"),
        proptest::option::of(git_state()),
        unique_entries(),
        unique_entries(),
        proptest::option::of(any::<[u8; 32]>()),
        proptest::collection::vec(cache_entry(), 0..3),
    )
        .prop_map(
            |(
                job,
                root,
                parent,
                created_at,
                by,
                (os, arch),
                git,
                files,
                harness,
                env,
                mut caches,
            )| {
                let mut seen = std::collections::HashSet::new();
                caches.retain(|c| seen.insert(c.path.clone()));
                SnapshotManifest {
                    version: MANIFEST_VERSION,
                    job_id: JobId::from_bytes(job),
                    root,
                    parent: parent.map(SnapshotId::from_bytes),
                    created_at,
                    created_by: NodeId::from_bytes(by),
                    platform: Platform { os, arch },
                    git,
                    files,
                    harness_state: HarnessState { entries: harness },
                    env_spec: env.map(EnvSpecId::from_bytes),
                    caches,
                }
            },
        )
}

proptest! {
    #[test]
    fn encode_decode_round_trips_and_the_hash_is_stable(m in manifest()) {
        let bytes = m.encode().unwrap();
        let back = SnapshotManifest::decode(&bytes).unwrap();
        prop_assert_eq!(&back, &m);
        prop_assert_eq!(back.encode().unwrap(), bytes.clone());
        prop_assert_eq!(back.id().unwrap(), SnapshotId::of(&bytes));
        prop_assert_eq!(m.id().unwrap(), back.id().unwrap());
    }
}

fn fixed_manifest() -> Result<SnapshotManifest, PathError> {
    Ok(SnapshotManifest {
        version: MANIFEST_VERSION,
        job_id: JobId::from_bytes([1; 16]),
        root: AbsPath::parse("/Users/efebarandurmaz/fabric/jobs/x/work")?,
        parent: None,
        created_at: 1_760_000_000_000,
        created_by: NodeId::from_bytes([2; 32]),
        platform: Platform {
            os: "macos".into(),
            arch: "arm64".into(),
        },
        git: Some(GitState {
            bundle: vec![ChunkId::of(b"bundle")],
            bundle_base: None,
            refs: vec![("refs/heads/main".into(), "a".repeat(40))],
            head: GitHead::Branch("main".into()),
            index_commit: "b".repeat(40),
            worktree_commit: "c".repeat(40),
            worktree_mtimes: vec![(RelPath::parse("src/lib.rs")?, 7)],
            config: vec![("push.default".into(), "simple".into())],
            exclude: None,
        }),
        files: vec![Entry {
            rel_path: RelPath::parse("node_modules/left-pad/index.js")?,
            kind: EntryKind::File,
            mode: 0o644,
            mtime_ns: 9,
            size: 3,
            chunks: vec![ChunkId::of(b"abc")],
        }],
        harness_state: HarnessState {
            entries: vec![Entry {
                rel_path: RelPath::parse("projects/-x/session.jsonl")?,
                kind: EntryKind::File,
                mode: 0o600,
                mtime_ns: 11,
                size: 0,
                chunks: vec![],
            }],
        },
        env_spec: Some(EnvSpecId::of(b"spec")),
        caches: vec![CacheEntry {
            path: RelPath::parse("node_modules")?,
            class: CacheClass::Deps,
            carried: true,
            key: CarryKey::Node {
                node_major: "22".into(),
                package_manager: "npm".into(),
                pm_major: "10".into(),
                pnpm_store_path: None,
            },
            partial: false,
        }],
    })
}

/// Pins format version 1. A field added, removed or reordered changes this
/// hash and must come with a version bump.
#[test]
fn format_version_one_hash_is_pinned() {
    let id = fixed_manifest().unwrap().id().unwrap().to_string();
    assert_eq!(
        id,
        "9153450fec8d204a97bfe3d789775a150437b3acf32a49beb7476134bb0d2204"
    );
}

#[test]
fn duplicate_paths_and_chunks_on_dirs_are_refused() {
    let mut m = fixed_manifest().unwrap();
    m.files.push(m.files[0].clone());
    assert!(matches!(
        m.encode(),
        Err(ManifestError::DuplicatePath { index: 1 })
    ));
    let mut m = fixed_manifest().unwrap();
    m.files[0].kind = EntryKind::Dir;
    assert!(matches!(
        m.encode(),
        Err(ManifestError::ChunksOnNonFile { index: 0 })
    ));
}

#[test]
fn a_bad_path_on_the_wire_does_not_decode() {
    let bytes = fixed_manifest().unwrap().encode().unwrap();
    let good = postcard::to_stdvec("node_modules/left-pad/index.js").unwrap();
    let bad = postcard::to_stdvec("node_modules/../index.js").unwrap();
    let pos = bytes
        .windows(good.len())
        .position(|w| w == good)
        .expect("path is in the bytes");
    let mut patched = bytes[..pos].to_vec();
    patched.extend_from_slice(&bad);
    patched.extend_from_slice(&bytes[pos + good.len()..]);
    assert!(matches!(
        SnapshotManifest::decode(&patched),
        Err(ManifestError::Codec(_))
    ));
}

#[test]
fn unknown_version_and_oversize_are_refused() {
    let mut m = fixed_manifest().unwrap();
    m.version = 2;
    assert!(matches!(m.encode(), Err(ManifestError::UnknownVersion(2))));
    let big = vec![0u8; fabric_core::snapshot::MAX_MANIFEST_LEN + 1];
    assert!(matches!(
        SnapshotManifest::decode(&big),
        Err(ManifestError::TooLarge { .. })
    ));
}
