//! The snapshot manifest codec: round trip for arbitrary manifests and a
//! pinned hash, so a change to the wire format is a failing test, not a
//! surprise on the other machine.

use std::error::Error;

use fabric_core::codec::{CodecError, VersionedError};
use fabric_core::env::{RunWhen, SetupArg, SetupStep};
use fabric_core::git::{BranchName, GitConfigKey, GitConfigValue, GitOid, RefName};
use fabric_core::harness::ProgramName;
use fabric_core::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, Entry, EntryKind, GitHead, GitState,
    HarnessState, LeaveReason, LeftBehind, MANIFEST_VERSION, ManifestError, Mode, Platform, Remake,
    SnapshotManifest,
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
        proptest::collection::vec(setup_step(), 1..3).prop_map(Remake::Setup),
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
        proptest::collection::vec((rel_path(), any::<u64>()), 0..3),
        proptest::collection::vec(config_entry(), 0..3),
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
                refs: sorted_unique(refs, |(name, _)| name.clone()),
                head,
                index_commit,
                worktree_commit,
                worktree_mtimes: sorted_unique(worktree_mtimes, |(path, _)| path.clone()),
                config: sorted_unique(config, |(key, _)| key.clone()),
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
        tree(),
        tree(),
        proptest::option::of(any::<[u8; 32]>()),
        (
            proptest::collection::vec(cache_entry(), 0..3),
            left_behind(),
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
                git,
                files,
                harness,
                env,
                (caches, left_behind),
            )| {
                let caches = sorted_unique(caches, |c| c.path.clone());
                SnapshotManifest {
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
        // created_at follows the version, the job id, the root and the parent.
        let start = 1
            + 16
            + postcard::to_stdvec(&m.root).unwrap().len()
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
            refs: vec![(
                RefName::parse("refs/heads/main")?,
                GitOid::parse(&"a".repeat(40))?,
            )],
            head: GitHead::Branch(BranchName::parse("main")?),
            index_commit: GitOid::parse(&"b".repeat(40))?,
            worktree_commit: GitOid::parse(&"c".repeat(40))?,
            worktree_mtimes: vec![(RelPath::parse("src/lib.rs")?, 7)],
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
            remake: Remake::Setup(vec![SetupStep {
                program: ProgramName::parse("npm")?,
                args: vec![SetupArg::parse("ci")?],
                cwd: None,
                when: RunWhen::CachesMissing,
            }]),
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
/// rows gained what a target decides them by (2026-10-21).
#[test]
fn format_version_two_hash_is_pinned() {
    let id = fixed_manifest().unwrap().id().unwrap().to_string();
    assert_eq!(
        id,
        "9903bed2bb2d4168f3de5a36ec6cf61220e95b68affd4b246977eca189531a87"
    );
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
    let mut m = fixed_manifest().unwrap();
    let git = m.git.as_mut().unwrap();
    git.config.push(git.config[0].clone());
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
    m.caches[0].remake = Remake::Setup(Vec::new());
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
    let mut bytes = fixed_manifest().unwrap().encode().unwrap();
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
