//! The snapshot manifest codec: round trip for arbitrary manifests and a
//! pinned hash per format version, so a change to the wire format is a
//! failing test, not a surprise on the other machine, and a manifest an
//! older build wrote keeps its id.

use std::error::Error;

use fabric_core::codec::{CodecError, VersionedError};
use fabric_core::env::{RunWhen, SetupArg, SetupStep};
use fabric_core::git::{BranchName, GitConfigKey, GitConfigValue, GitOid, RefName};
use fabric_core::harness::ProgramName;
use fabric_core::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, Entry, EntryKind, GitHead, GitState,
    HarnessState, LEGACY_MANIFEST_VERSION, LeaveReason, LeftBehind, MANIFEST_VERSION,
    ManifestError, Mode, PathMeta, Platform, Remake, SnapshotManifest,
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

fn any_mode() -> impl Strategy<Value = Mode> {
    (0u16..=0o777).prop_filter_map("mode", Mode::new)
}

fn path_meta() -> impl Strategy<Value = PathMeta> {
    (any_mode(), any::<u64>()).prop_map(|(mode, mtime_ns)| PathMeta { mode, mtime_ns })
}

fn chunk() -> impl Strategy<Value = ChunkId> {
    any::<[u8; 32]>().prop_map(ChunkId::from_bytes)
}

fn entry() -> impl Strategy<Value = Entry> {
    (
        rel_path(),
        prop_oneof![
            (any::<u64>(), proptest::collection::vec(chunk(), 0..3))
                .prop_map(|(size, chunks)| EntryKind::File { size, chunks }),
            Just(EntryKind::Dir),
            "[a-z/.]{1,12}".prop_map(|target| EntryKind::Symlink { target }),
        ],
        (0u16..=0o777).prop_filter_map("mode", Mode::new),
        any::<u64>(),
    )
        .prop_map(|(rel_path, kind, mode, mtime_ns)| Entry {
            rel_path,
            kind,
            mode,
            mtime_ns,
        })
}

/// A tree the rules accept: sorted by path, no path below a file or link.
fn tree() -> impl Strategy<Value = Vec<Entry>> {
    proptest::collection::vec(entry(), 0..6).prop_map(|mut entries| {
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        entries.dedup_by(|a, b| a.rel_path == b.rel_path);
        let mut kept: Vec<Entry> = Vec::with_capacity(entries.len());
        for e in entries {
            let below_non_dir = kept.iter().any(|k| {
                !matches!(k.kind, EntryKind::Dir)
                    && e.rel_path != k.rel_path
                    && e.rel_path.starts_with(&k.rel_path)
            });
            if !below_non_dir {
                kept.push(e);
            }
        }
        kept
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
        Just(CarryKey::Unmeasured),
    ]
}

fn setup_step() -> impl Strategy<Value = SetupStep> {
    (
        "(npm|pnpm|uv|cargo)",
        proptest::collection::vec("[a-z-]{1,8}", 0..3),
        proptest::option::of(rel_path()),
        prop_oneof![Just(RunWhen::CachesMissing), Just(RunWhen::Always)],
    )
        .prop_filter_map("valid step", |(program, args, cwd, when)| {
            Some(SetupStep {
                program: ProgramName::parse(&program).ok()?,
                args: args
                    .iter()
                    .map(|a| SetupArg::parse(a))
                    .collect::<Result<_, _>>()
                    .ok()?,
                cwd,
                when,
            })
        })
}

fn remake() -> impl Strategy<Value = Remake> {
    prop_oneof![
        (proptest::collection::vec(setup_step(), 1..3), any::<bool>())
            .prop_map(|(steps, installed)| Remake::Setup { steps, installed }),
        Just(Remake::NextBuild),
        Just(Remake::Nothing),
    ]
}

fn left_behind() -> impl Strategy<Value = Vec<LeftBehind>> {
    proptest::collection::vec(
        (
            rel_path(),
            prop_oneof![Just(LeaveReason::Secret), Just(LeaveReason::Excluded)],
        )
            .prop_map(|(path, reason)| LeftBehind { path, reason }),
        0..3,
    )
    .prop_map(|items| sorted_unique(items, |l| l.path.clone()))
}

fn cache_entry() -> impl Strategy<Value = CacheEntry> {
    (
        rel_path(),
        prop_oneof![
            Just(CacheClass::Deps),
            Just(CacheClass::Build),
            Just(CacheClass::ToolCache)
        ],
        prop_oneof![
            Just(CarryState::Carried),
            Just(CarryState::Partial),
            Just(CarryState::NotCarried)
        ],
        carry_key(),
        remake(),
        any::<u64>(),
    )
        .prop_map(
            |(path, class, carry, key, remake, native_files)| CacheEntry {
                path,
                class,
                carry,
                key,
                remake,
                native_files,
            },
        )
}

/// Sorts `items` by `key` and keeps a repeated key's items in their order, as capture does for config.
fn sorted<T, K: Ord>(mut items: Vec<T>, key: impl Fn(&T) -> K) -> Vec<T> {
    items.sort_by_key(|a| key(a));
    items
}

/// Sorts `items` by `key` and drops a repeated key, as capture does.
fn sorted_unique<T, K: Ord>(mut items: Vec<T>, key: impl Fn(&T) -> K) -> Vec<T> {
    items.sort_by_key(|a| key(a));
    items.dedup_by(|a, b| key(a) == key(b));
    items
}

fn oid() -> impl Strategy<Value = GitOid> {
    "[0-9a-f]{40}".prop_filter_map("valid oid", |s| GitOid::parse(&s).ok())
}

fn ref_name() -> impl Strategy<Value = RefName> {
    "refs/[a-z]{1,4}(/[a-z0-9]{1,4}){0,2}".prop_filter_map("valid ref", |s| RefName::parse(&s).ok())
}

fn config_entry() -> impl Strategy<Value = (GitConfigKey, GitConfigValue)> {
    (
        prop_oneof![
            "remote\\.[a-z]{1,5}\\.(url|pushurl|fetch)",
            "branch\\.[a-z]{1,5}\\.(remote|merge|rebase)",
            Just("push.default".to_owned()),
            Just("core.ignorecase".to_owned()),
            Just("core.autocrlf".to_owned()),
            Just("core.symlinks".to_owned()),
        ],
        "[a-z:/.+*]{0,10}",
    )
        .prop_filter_map("valid config entry", |(k, v)| {
            Some((
                GitConfigKey::parse(&k).ok()?,
                GitConfigValue::parse(&v).ok()?,
            ))
        })
}

fn git_state() -> impl Strategy<Value = GitState> {
    (
        proptest::collection::vec(chunk(), 1..3),
        proptest::option::of(any::<[u8; 32]>().prop_map(SnapshotId::from_bytes)),
        proptest::collection::vec((ref_name(), oid()), 0..3),
        prop_oneof![
            "[a-z-]{1,8}"
                .prop_filter_map("valid branch", |s| BranchName::parse(&s).ok())
                .prop_map(GitHead::Branch),
            oid().prop_map(GitHead::Detached)
        ],
        oid(),
        oid(),
        proptest::collection::vec((rel_path(), any::<u64>(), any_mode()), 0..3),
        proptest::collection::vec(config_entry(), 0..3),
        proptest::option::of(proptest::collection::vec(chunk(), 1..2)),
        proptest::collection::vec(ref_name(), 0..2),
    )
        .prop_map(
            |(
                bundle,
                bundle_base,
                refs,
                head,
                index_commit,
                worktree_commit,
                worktree,
                config,
                exclude,
                symbolic,
            )| {
                let refs = sorted_unique(refs, |(name, _)| name.clone());
                // Each symbolic ref names a ref of `refs` and is not one.
                let symbolic_refs = match refs.first() {
                    Some((target, _)) => sorted_unique(
                        symbolic
                            .into_iter()
                            .filter(|name| refs.iter().all(|(n, _)| n != name))
                            .map(|name| (name, target.clone()))
                            .collect(),
                        |(name, _)| name.clone(),
                    ),
                    None => Vec::new(),
                };
                let worktree = sorted_unique(worktree, |(path, _, _)| path.clone());
                GitState {
                    bundle,
                    bundle_base,
                    refs,
                    symbolic_refs,
                    head,
                    index_commit,
                    worktree_commit,
                    worktree_mtimes: worktree.iter().map(|(p, t, _)| (p.clone(), *t)).collect(),
                    worktree_modes: worktree.into_iter().map(|(p, _, m)| (p, m)).collect(),
                    config: sorted(config, |(key, _)| key.clone()),
                    exclude,
                }
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
        tree(),
        tree(),
        proptest::option::of(any::<[u8; 32]>()),
        (
            proptest::collection::vec(cache_entry(), 0..3),
            left_behind(),
            proptest::option::of(path_meta()),
            proptest::option::of(path_meta()),
        ),
    )
        .prop_map(
            |(
                job,
                root,
                parent,
                created_at,
                by,
                (os, arch),
                mut git,
                files,
                harness,
                env,
                (caches, left_behind, root_meta, harness_root),
            )| {
                let caches = sorted_unique(caches, |c| c.path.clone());
                // A path is in `files` or in the working tree, never both,
                // and the two nest only below a directory in `files`.
                let apart = |path: &RelPath| {
                    files.iter().all(|e| {
                        !e.rel_path.starts_with(path)
                            && (matches!(e.kind, EntryKind::Dir) || !path.starts_with(&e.rel_path))
                    })
                };
                if let Some(git) = git.as_mut() {
                    git.worktree_mtimes.retain(|(path, _)| apart(path));
                    git.worktree_modes.retain(|(path, _)| apart(path));
                    // Format version 2 has no field for these.
                    if root_meta.is_none() {
                        git.symbolic_refs.clear();
                        git.worktree_modes.clear();
                    }
                }
                SnapshotManifest {
                    job_id: JobId::from_bytes(job),
                    root,
                    root_meta,
                    parent: parent.map(SnapshotId::from_bytes),
                    created_at,
                    created_by: NodeId::from_bytes(by),
                    platform: Platform { os, arch },
                    git,
                    files,
                    harness_state: HarnessState {
                        entries: harness,
                        root: harness_root.filter(|_| root_meta.is_some()),
                    },
                    env_spec: env.map(EnvSpecId::from_bytes),
                    caches,
                    left_behind,
                }
            },
        )
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

    /// An overlong varint, in the version prefix or in a field, decodes to
    /// the same manifest from other bytes; `id()` would then differ from
    /// blake3 of the bytes, so the bytes are refused.
    #[test]
    fn an_overlong_varint_in_a_manifest_does_not_decode(m in manifest()) {
        let bytes = m.encode().unwrap();
        let mut version = overlong(&bytes[..1]);
        version.extend_from_slice(&bytes[1..]);
        prop_assert!(matches!(
            SnapshotManifest::decode(&version),
            Err(ManifestError::Codec(VersionedError::Codec(CodecError::NonCanonical)))
        ));
        // created_at follows the version, the job id, the root, in
        // version 3 the root's mode and mtime, and the parent.
        let root_meta = match m.root_meta {
            Some(_) => postcard::to_stdvec(&m.root_meta).unwrap().len(),
            None => 0,
        };
        let start = 1
            + 16
            + postcard::to_stdvec(&m.root).unwrap().len()
            + root_meta
            + postcard::to_stdvec(&m.parent).unwrap().len();
        let created_at = postcard::to_stdvec(&m.created_at).unwrap();
        let mut field = bytes[..start].to_vec();
        field.extend(overlong(&created_at));
        field.extend_from_slice(&bytes[start + created_at.len()..]);
        prop_assert!(matches!(
            SnapshotManifest::decode(&field),
            Err(ManifestError::Codec(_))
        ));
    }
}

fn mode(bits: u16) -> Result<Mode, Box<dyn Error>> {
    Mode::new(bits).ok_or_else(|| format!("mode 0o{bits:o} has bits above 0o777").into())
}

fn fixed_manifest() -> Result<SnapshotManifest, Box<dyn Error>> {
    Ok(SnapshotManifest {
        job_id: JobId::from_bytes([1; 16]),
        root: AbsPath::parse("/Users/me/fabric/jobs/x/work")?,
        root_meta: None,
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
            refs: vec![(
                RefName::parse("refs/heads/main")?,
                GitOid::parse(&"a".repeat(40))?,
            )],
            symbolic_refs: Vec::new(),
            head: GitHead::Branch(BranchName::parse("main")?),
            index_commit: GitOid::parse(&"b".repeat(40))?,
            worktree_commit: GitOid::parse(&"c".repeat(40))?,
            worktree_mtimes: vec![(RelPath::parse("src/lib.rs")?, 7)],
            worktree_modes: Vec::new(),
            config: vec![(
                GitConfigKey::parse("push.default")?,
                GitConfigValue::parse("simple")?,
            )],
            exclude: None,
        }),
        files: vec![Entry {
            rel_path: RelPath::parse("node_modules/left-pad/index.js")?,
            kind: EntryKind::File {
                size: 3,
                chunks: vec![ChunkId::of(b"abc")],
            },
            mode: mode(0o644)?,
            mtime_ns: 9,
        }],
        harness_state: HarnessState {
            entries: vec![Entry {
                rel_path: RelPath::parse("projects/-x/session.jsonl")?,
                kind: EntryKind::File {
                    size: 0,
                    chunks: vec![],
                },
                mode: mode(0o600)?,
                mtime_ns: 11,
            }],
            root: None,
        },
        env_spec: Some(EnvSpecId::of(b"spec")),
        caches: vec![CacheEntry {
            path: RelPath::parse("node_modules")?,
            class: CacheClass::Deps,
            carry: CarryState::Carried,
            key: CarryKey::Node {
                node_major: "22".into(),
                package_manager: "npm".into(),
                pm_major: "10".into(),
                pnpm_store_path: None,
            },
            remake: Remake::Setup {
                steps: vec![SetupStep {
                    program: ProgramName::parse("npm")?,
                    args: vec![SetupArg::parse("ci")?],
                    cwd: None,
                    when: RunWhen::CachesMissing,
                }],
                installed: true,
            },
            native_files: 0,
        }],
        left_behind: vec![LeftBehind {
            path: RelPath::parse(".env")?,
            reason: LeaveReason::Secret,
        }],
    })
}

/// Pins format version 2. A field added, removed or reordered changes this
/// hash and must come with a version bump, as version 2 did when the cache
/// rows gained what a target decides them by (2026-10-21). `installed`
/// joined `Remake::Setup` on 2026-10-22, before version 2 was merged or
/// written by any released build, so it is part of version 2. Since
/// version 3 this is the hash of a manifest with no `root_meta`, which a
/// version 2 writer made: the same hash proves such a snapshot keeps its id.
#[test]
fn format_version_two_hash_is_pinned() {
    let m = fixed_manifest().unwrap();
    assert_eq!(m.version(), LEGACY_MANIFEST_VERSION);
    let id = m.id().unwrap().to_string();
    assert_eq!(
        id,
        "67d16a515a83e7bec72705376208b8f039045c5428e7f95bc862df7724b5c765"
    );
}

/// The fixed manifest as version 3 writes it: the workspace root and the
/// harness-state root with their mode and mtime, a file of `W` with its
/// bits and `refs/remotes/origin/HEAD` as a symbolic ref.
fn fixed_manifest_v3() -> Result<SnapshotManifest, Box<dyn Error>> {
    let mut m = fixed_manifest()?;
    m.root_meta = Some(PathMeta {
        mode: mode(0o700)?,
        mtime_ns: 13,
    });
    m.harness_state.root = Some(PathMeta {
        mode: mode(0o700)?,
        mtime_ns: 17,
    });
    let git = m.git.as_mut().ok_or("git")?;
    git.refs.push((
        RefName::parse("refs/remotes/origin/main")?,
        GitOid::parse(&"a".repeat(40))?,
    ));
    git.symbolic_refs = vec![(
        RefName::parse("refs/remotes/origin/HEAD")?,
        RefName::parse("refs/remotes/origin/main")?,
    )];
    git.worktree_modes = vec![(RelPath::parse("src/lib.rs")?, mode(0o600)?)];
    Ok(m)
}

/// Pins format version 3 (M2-04, M2-05, M2-06), as the version 2 hash
/// above pins version 2.
#[test]
fn format_version_three_hash_is_pinned() {
    let m = fixed_manifest_v3().unwrap();
    assert_eq!(m.version(), MANIFEST_VERSION);
    let bytes = m.encode().unwrap();
    assert_eq!(bytes[0], u8::try_from(MANIFEST_VERSION).unwrap());
    assert_eq!(SnapshotManifest::decode(&bytes).unwrap(), m);
    assert_eq!(
        m.id().unwrap().to_string(),
        "ea8a01366aea4bba6bd828f5f2a79459cfc83d56dc7ec72221f74d13bc8e6ca0"
    );
}

/// Bytes a version 2 writer made decode, with none of what version 3
/// added, and encode to the same bytes, so the snapshot keeps its id.
#[test]
fn a_version_two_manifest_decodes_and_keeps_its_bytes() {
    let m = fixed_manifest().unwrap();
    let bytes = m.encode().unwrap();
    assert_eq!(bytes[0], u8::try_from(LEGACY_MANIFEST_VERSION).unwrap());
    let back = SnapshotManifest::decode(&bytes).unwrap();
    assert_eq!(back, m);
    assert_eq!(back.root_meta, None);
    assert_eq!(back.encode().unwrap(), bytes);
    assert_eq!(back.id().unwrap(), SnapshotId::of(&bytes));
}

/// Clears every version 3 field of a manifest but one.
type KeepOnly = fn(&mut SnapshotManifest);

/// A manifest with no `root_meta` is written as version 2, which has no
/// field for the rest of what version 3 added, so it holds none of it.
#[test]
fn a_manifest_without_root_meta_holds_no_version_three_field() {
    let v3 = fixed_manifest_v3().unwrap();
    let cases: [(&str, KeepOnly); 3] = [
        ("harness_state.root", |m| {
            m.git.as_mut().unwrap().symbolic_refs.clear();
            m.git.as_mut().unwrap().worktree_modes.clear();
        }),
        ("symbolic_refs", |m| {
            m.harness_state.root = None;
            m.git.as_mut().unwrap().worktree_modes.clear();
        }),
        ("worktree_modes", |m| {
            m.harness_state.root = None;
            m.git.as_mut().unwrap().symbolic_refs.clear();
        }),
    ];
    for (field, keep_only) in cases {
        let mut m = v3.clone();
        m.root_meta = None;
        keep_only(&mut m);
        assert!(
            matches!(m.encode(), Err(ManifestError::NotInVersion2 { field: f }) if f == field),
            "{field}"
        );
    }
}

/// Version 3 bytes always record the root; only version 2 leaves it out,
/// so one manifest has one encoding.
#[test]
fn version_three_bytes_without_root_meta_do_not_decode() {
    let m = fixed_manifest_v3().unwrap();
    let bytes = m.encode().unwrap();
    let at = 1 + 16 + postcard::to_stdvec(&m.root).unwrap().len();
    let meta = postcard::to_stdvec(&m.root_meta).unwrap();
    assert_eq!(&bytes[at..at + meta.len()], &meta[..]);
    let mut patched = bytes[..at].to_vec();
    patched.push(0);
    patched.extend_from_slice(&bytes[at + meta.len()..]);
    assert!(matches!(
        SnapshotManifest::decode(&patched),
        Err(ManifestError::MissingRootMeta)
    ));
}

/// A symbolic ref points at a ref the bundle carries, and is not one.
#[test]
fn a_symbolic_ref_must_point_at_a_ref_and_not_be_one() {
    let mut m = fixed_manifest_v3().unwrap();
    m.git.as_mut().unwrap().symbolic_refs[0].1 =
        RefName::parse("refs/remotes/origin/gone").unwrap();
    assert!(matches!(
        m.encode(),
        Err(ManifestError::SymbolicRefTargetMissing { name, target })
            if name.as_str() == "refs/remotes/origin/HEAD"
                && target.as_str() == "refs/remotes/origin/gone"
    ));
    let mut m = fixed_manifest_v3().unwrap();
    m.git.as_mut().unwrap().symbolic_refs[0].0 = RefName::parse("refs/heads/main").unwrap();
    assert!(matches!(
        m.encode(),
        Err(ManifestError::SymbolicRefIsRef { name }) if name.as_str() == "refs/heads/main"
    ));
}

/// A mode belongs to a file of the working-tree commit.
#[test]
fn a_mode_for_a_path_outside_the_working_tree_is_refused() {
    let mut m = fixed_manifest_v3().unwrap();
    m.git.as_mut().unwrap().worktree_modes[0].0 = RelPath::parse("src/main.rs").unwrap();
    assert!(matches!(
        m.encode(),
        Err(ManifestError::ModeOutsideWorktree { path }) if path.as_str() == "src/main.rs"
    ));
}

fn dir(path: &str) -> Result<Entry, Box<dyn Error>> {
    Ok(Entry {
        rel_path: RelPath::parse(path)?,
        kind: EntryKind::Dir,
        mode: mode(0o755)?,
        mtime_ns: 1,
    })
}

#[test]
fn a_repeated_or_unsorted_path_is_refused() {
    let mut m = fixed_manifest().unwrap();
    m.files.push(m.files[0].clone());
    assert!(matches!(
        m.encode(),
        Err(ManifestError::OutOfOrder {
            list: "files",
            index: 1
        })
    ));
    let mut m = fixed_manifest().unwrap();
    m.files = vec![dir("b").unwrap(), dir("a").unwrap()];
    assert!(matches!(
        m.encode(),
        Err(ManifestError::OutOfOrder {
            list: "files",
            index: 1
        })
    ));
    let mut m = fixed_manifest().unwrap();
    m.caches.push(m.caches[0].clone());
    assert!(matches!(
        m.encode(),
        Err(ManifestError::OutOfOrder {
            list: "caches",
            index: 1
        })
    ));
    let mut m = fixed_manifest().unwrap();
    m.left_behind.push(m.left_behind[0].clone());
    assert!(matches!(
        m.encode(),
        Err(ManifestError::OutOfOrder {
            list: "left_behind",
            index: 1
        })
    ));
}

/// `config` is sorted by key, but a key git holds more than once keeps one
/// entry per value in git's order, such as a second fetch refspec; a key
/// below the one before it is still refused.
#[test]
fn a_repeated_config_key_keeps_its_values_and_an_unsorted_one_is_refused() {
    let mut m = fixed_manifest().unwrap();
    let fetch = GitConfigKey::parse("remote.origin.fetch").unwrap();
    m.git.as_mut().unwrap().config = vec![
        (
            fetch.clone(),
            GitConfigValue::parse("+refs/heads/*:refs/remotes/origin/*").unwrap(),
        ),
        (
            fetch,
            GitConfigValue::parse("+refs/pull/*/head:refs/remotes/origin/pr/*").unwrap(),
        ),
    ];
    let bytes = m.encode().unwrap();
    assert_eq!(SnapshotManifest::decode(&bytes).unwrap(), m);
    let git = m.git.as_mut().unwrap();
    git.config.insert(
        0,
        (
            GitConfigKey::parse("push.default").unwrap(),
            GitConfigValue::parse("simple").unwrap(),
        ),
    );
    assert!(matches!(
        m.encode(),
        Err(ManifestError::OutOfOrder {
            list: "config",
            index: 1
        })
    ));
}

/// A cache row that says setup steps remake it must name one: a target
/// would otherwise plan a rebuild that runs nothing and drop the bytes.
#[test]
fn a_remake_with_no_setup_step_is_refused() {
    let mut m = fixed_manifest().unwrap();
    m.caches[0].remake = Remake::Setup {
        steps: Vec::new(),
        installed: true,
    };
    assert!(matches!(
        m.encode(),
        Err(ManifestError::EmptyRemake { index: 0 })
    ));
}

/// ADR 0004: setuid, setgid and sticky bits never travel. `Mode` cannot hold
/// them, so the only place they can appear is on the wire, where the
/// manifest does not decode rather than being masked.
#[test]
fn a_mode_above_0o777_on_the_wire_does_not_decode() {
    assert_eq!(Mode::new(0o4755), None);
    let bytes = fixed_manifest().unwrap().encode().unwrap();
    // The first file's mode 0o644 (varint a4 03) followed by its mtime 9.
    let good = [0xa4, 0x03, 0x09];
    let positions: Vec<usize> = bytes
        .windows(good.len())
        .enumerate()
        .filter_map(|(i, w)| (w == good).then_some(i))
        .collect();
    assert_eq!(positions.len(), 1, "the mode bytes must be unambiguous");
    let mut patched = bytes.clone();
    // 0o4755 as a varint: ed 13.
    patched.splice(positions[0]..positions[0] + 2, [0xed, 0x13]);
    assert!(matches!(
        SnapshotManifest::decode(&patched),
        Err(ManifestError::Codec(_))
    ));
}

/// Defence in depth for the file writer: a tree that puts `a/config` under a
/// symlink `a` never reaches it, on macOS where there is no `RESOLVE_BENEATH`
/// included.
#[test]
fn an_entry_below_a_file_or_symlink_is_refused() {
    let mut m = fixed_manifest().unwrap();
    m.files = vec![
        Entry {
            rel_path: RelPath::parse("a").unwrap(),
            kind: EntryKind::Symlink {
                target: "../../.git".into(),
            },
            mode: mode(0o777).unwrap(),
            mtime_ns: 1,
        },
        dir("a/config").unwrap(),
    ];
    assert!(matches!(
        m.encode(),
        Err(ManifestError::UnderNonDir { index: 1 })
    ));
    m.files[0].kind = EntryKind::File {
        size: 0,
        chunks: vec![],
    };
    assert!(matches!(
        m.encode(),
        Err(ManifestError::UnderNonDir { index: 1 })
    ));
    m.files[0].kind = EntryKind::Dir;
    assert!(m.encode().is_ok());
}

/// Returns `bytes` with the one string `good` replaced by `bad`.
fn patch(bytes: &[u8], good: &str, bad: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let good = postcard::to_stdvec(good)?;
    let bad = postcard::to_stdvec(bad)?;
    let pos = bytes
        .windows(good.len())
        .position(|w| w == good)
        .ok_or("string is not in the bytes")?;
    let mut out = bytes[..pos].to_vec();
    out.extend_from_slice(&bad);
    out.extend_from_slice(&bytes[pos + good.len()..]);
    Ok(out)
}

/// Returns the fixed manifest's bytes with the one string `good` replaced by `bad`.
fn patched(good: &str, bad: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    patch(&fixed_manifest()?.encode()?, good, bad)
}

#[test]
fn a_bad_path_on_the_wire_does_not_decode() {
    let bytes = patched("node_modules/left-pad/index.js", "node_modules/../index.js").unwrap();
    assert!(matches!(
        SnapshotManifest::decode(&bytes),
        Err(ManifestError::Codec(_))
    ));
}

/// The config projection is an allowlist on both ends: a peer manifest that
/// names `credential.helper` never exists as a value on the receiving node.
#[test]
fn a_denied_config_key_on_the_wire_does_not_decode() {
    let bytes = patched("push.default", "credential.helper").unwrap();
    assert!(matches!(
        SnapshotManifest::decode(&bytes),
        Err(ManifestError::Codec(_))
    ));
    let bytes = patched("refs/heads/main", "refs/heads/a..b").unwrap();
    assert!(matches!(
        SnapshotManifest::decode(&bytes),
        Err(ManifestError::Codec(_))
    ));
}

/// ADR 0004 strips userinfo from remote URLs at capture. The receiving node
/// enforces the same line, so a token in `remote.origin.url` never decodes
/// and is never written into a target's `.git/config`.
#[test]
fn a_remote_url_with_userinfo_on_the_wire_does_not_decode() {
    let bytes = patched("push.default", "remote.origin.url").unwrap();
    let clean = patch(&bytes, "simple", "https://github.com/x/y.git").unwrap();
    assert!(SnapshotManifest::decode(&clean).is_ok());
    let token = patch(
        &bytes,
        "simple",
        "https://efe:ghp_secret123@github.com/x/y.git",
    )
    .unwrap();
    assert!(matches!(
        SnapshotManifest::decode(&token),
        Err(ManifestError::SecretInConfig { index: 0 })
    ));
}

/// SHA-1 and SHA-256 repositories do not interoperate, so a manifest whose
/// ids mix the two widths could not be restored into any repository.
#[test]
fn mixed_object_id_widths_are_refused() {
    let sha256 = GitOid::parse(&"d".repeat(64)).unwrap();
    let mixes: [fn(&mut GitState, GitOid); 4] = [
        |g, oid| g.refs[0].1 = oid,
        |g, oid| g.head = GitHead::Detached(oid),
        |g, oid| g.index_commit = oid,
        |g, oid| g.worktree_commit = oid,
    ];
    for mix in mixes {
        let mut m = fixed_manifest().unwrap();
        mix(m.git.as_mut().unwrap(), sha256.clone());
        assert!(matches!(m.encode(), Err(ManifestError::MixedObjectFormat)));
    }
    let mut m = fixed_manifest().unwrap();
    let git = m.git.as_mut().unwrap();
    git.refs[0].1 = sha256.clone();
    git.head = GitHead::Detached(sha256.clone());
    git.index_commit = sha256.clone();
    git.worktree_commit = sha256;
    let bytes = m.encode().unwrap();
    assert_eq!(SnapshotManifest::decode(&bytes).unwrap(), m);
}

/// Restore writes the working tree first and `files` with exclusive
/// creates, so a path in both would publish and never restore.
#[test]
fn a_path_in_files_and_the_working_tree_is_refused() {
    let mut m = fixed_manifest().unwrap();
    let path = m.files[0].rel_path.clone();
    let git = m.git.as_mut().unwrap();
    git.worktree_mtimes.push((path.clone(), 0));
    git.worktree_mtimes.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(matches!(
        m.encode(),
        Err(ManifestError::InBothTrees { path: p }) if p == path
    ));
}

/// Restore checks out the working tree before it creates `files`, so a
/// `files` entry below a working-tree file, or a `files` file or symlink
/// above a working-tree path, never restores; a `files` directory above
/// one is the parent the two trees share.
#[test]
fn a_files_entry_nested_with_a_working_tree_path_is_refused() {
    // The fixed manifest's working tree holds `src/lib.rs`.
    let mut m = fixed_manifest().unwrap();
    let nest = |m: &SnapshotManifest, files: &str, worktree: &str| {
        matches!(m.encode(), Err(ManifestError::TreesNest { files: f, worktree: w })
            if f.as_str() == files && w.as_str() == worktree)
    };
    m.files = vec![dir("src/lib.rs/x").unwrap()];
    assert!(nest(&m, "src/lib.rs/x", "src/lib.rs"));
    m.files = vec![Entry {
        rel_path: RelPath::parse("src").unwrap(),
        kind: EntryKind::File {
            size: 0,
            chunks: vec![],
        },
        mode: mode(0o644).unwrap(),
        mtime_ns: 1,
    }];
    assert!(nest(&m, "src", "src/lib.rs"));
    m.files[0].kind = EntryKind::Symlink {
        target: "elsewhere".into(),
    };
    assert!(nest(&m, "src", "src/lib.rs"));
    m.files = vec![dir("src").unwrap(), dir("src/target").unwrap()];
    let bytes = m.encode().unwrap();
    assert_eq!(SnapshotManifest::decode(&bytes).unwrap(), m);
}

#[test]
fn trailing_bytes_do_not_decode() {
    let mut bytes = fixed_manifest().unwrap().encode().unwrap();
    bytes.push(0);
    assert!(matches!(
        SnapshotManifest::decode(&bytes),
        Err(ManifestError::Codec(VersionedError::Codec(
            CodecError::TrailingBytes(1)
        )))
    ));
}

#[test]
fn unknown_version_and_oversize_are_refused() {
    let mut bytes = fixed_manifest_v3().unwrap().encode().unwrap();
    assert_eq!(bytes[0], u8::try_from(MANIFEST_VERSION).unwrap());
    bytes[0] = 1;
    assert!(matches!(
        SnapshotManifest::decode(&bytes),
        Err(ManifestError::Codec(VersionedError::UnknownVersion(1)))
    ));
    let big = vec![0u8; fabric_core::snapshot::MAX_MANIFEST_LEN + 1];
    assert!(matches!(
        SnapshotManifest::decode(&big),
        Err(ManifestError::TooLarge { .. })
    ));
}
