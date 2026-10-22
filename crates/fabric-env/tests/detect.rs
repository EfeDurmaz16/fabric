//! Detection over fixture and scratch workspaces: what each file gives,
//! which source wins, and what the override file changes.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use fabric_core::env::{
    EnvVarName, PinError, RunWhen, SetupArg, Tool, ToolName, ToolSource, ToolVersion,
};
use fabric_core::path::{PathError, RelPath};
use fabric_core::secret::{Scope, SecretPolicy};
use fabric_core::snapshot::CacheClass;
use fabric_env::detect::{
    CacheOrigin, CachePath, Completion, DetectError, Detection, Ecosystem, ManagerChoice,
    ManagerSource, NodeManager, detect,
};
use fabric_env::overrides::OverrideError;
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Writes `files` into a fresh directory.
fn workspace(files: &[(&str, &str)]) -> io::Result<TempDir> {
    let dir = tempfile::tempdir()?;
    for (path, text) in files {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full, text)?;
    }
    Ok(dir)
}

/// Each setup step as one line: the program, then its arguments.
fn commands(detection: &Detection) -> Vec<String> {
    detection
        .spec()
        .setup
        .iter()
        .map(|step| {
            std::iter::once(step.program.as_str())
                .chain(step.args.iter().map(SetupArg::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn tool(name: &str, version: &str, source: ToolSource) -> Result<Tool, PinError> {
    Ok(Tool {
        name: ToolName::parse(name)?,
        version: ToolVersion::parse(version)?,
        source,
    })
}

fn cache(
    path: &str,
    class: CacheClass,
    ecosystem: Ecosystem,
    completion: Completion,
) -> Result<CachePath, PathError> {
    Ok(CachePath {
        path: RelPath::parse(path)?,
        class,
        ecosystem,
        origin: CacheOrigin::Detected,
        completion,
    })
}

const PACKAGE_JSON: &str = r#"{"name":"x","version":"1.0.0"}"#;

/// Files to write, as `(path, contents)`.
type Files<'a> = &'a [(&'a str, &'a str)];

#[test]
fn a_pnpm_workspace_gives_a_frozen_install_and_node_modules() {
    let found = detect(&fixture("node-pnpm"), &[]).unwrap();
    assert_eq!(commands(&found), ["pnpm install --frozen-lockfile"]);
    assert_eq!(found.spec().setup[0].when, RunWhen::CachesMissing);
    assert_eq!(found.spec().setup[0].cwd, None);
    assert_eq!(
        found.caches(),
        [cache(
            "node_modules",
            CacheClass::Deps,
            Ecosystem::Node(NodeManager::Pnpm),
            Completion::Sentinel { filled_by: vec![0] }
        )
        .unwrap()]
    );
    assert_eq!(
        found.node_manager(),
        Some(ManagerChoice {
            manager: NodeManager::Pnpm,
            because: ManagerSource::PackageManagerField
        })
    );
    assert_eq!(
        found.spec().tools,
        [
            tool("node", "22", ToolSource::Detected).unwrap(),
            tool("pnpm", "11.23.0", ToolSource::Detected).unwrap()
        ]
    );
    assert_eq!(found.devcontainer(), None);
    assert!(found.spec().id().is_ok());
}

/// A resume must not rewrite `Cargo.lock`; without one there is nothing to keep.
#[test]
fn cargo_fetch_is_locked_exactly_when_a_lockfile_exists() {
    let found = detect(&fixture("rust-cargo"), &[]).unwrap();
    assert_eq!(commands(&found), ["cargo fetch --locked"]);
    assert_eq!(
        found.caches(),
        [cache(
            "target",
            CacheClass::Build,
            Ecosystem::Rust,
            Completion::Content
        )
        .unwrap()]
    );
    assert_eq!(
        found.spec().tools,
        [tool("rust", "1.97.1", ToolSource::Detected).unwrap()]
    );

    let library = workspace(&[("Cargo.toml", "[package]\nname = \"x\"\n")]).unwrap();
    assert_eq!(
        commands(&detect(library.path(), &[]).unwrap()),
        ["cargo fetch"]
    );
}

#[test]
fn each_lockfile_names_its_manager_and_install() {
    let cases: [(&[&str], NodeManager, &str); 9] = [
        (
            &["pnpm-lock.yaml"],
            NodeManager::Pnpm,
            "pnpm install --frozen-lockfile",
        ),
        (&["package-lock.json"], NodeManager::Npm, "npm ci"),
        (&["npm-shrinkwrap.json"], NodeManager::Npm, "npm ci"),
        (
            &["npm-shrinkwrap.json", "package-lock.json"],
            NodeManager::Npm,
            "npm ci",
        ),
        (
            &["yarn.lock"],
            NodeManager::YarnClassic,
            "yarn install --frozen-lockfile",
        ),
        (
            &["yarn.lock", ".yarnrc.yml"],
            NodeManager::YarnBerry,
            "yarn install --immutable",
        ),
        (
            &["bun.lock"],
            NodeManager::Bun,
            "bun install --frozen-lockfile",
        ),
        (
            &["bun.lockb"],
            NodeManager::Bun,
            "bun install --frozen-lockfile",
        ),
        // The text lockfile next to the binary one it replaced is one manager, not two.
        (
            &["bun.lock", "bun.lockb"],
            NodeManager::Bun,
            "bun install --frozen-lockfile",
        ),
    ];
    for (files, manager, command) in cases {
        let mut all = vec![("package.json", PACKAGE_JSON)];
        all.extend(files.iter().map(|&f| match f {
            ".yarnrc.yml" => (f, "nodeLinker: node-modules\n"),
            _ => (f, ""),
        }));
        let dir = workspace(&all).unwrap();
        let found = detect(dir.path(), &[]).unwrap();
        assert_eq!(commands(&found), [command], "{files:?}");
        assert_eq!(
            found.node_manager(),
            Some(ManagerChoice {
                manager,
                because: ManagerSource::Lockfile
            }),
            "{files:?}"
        );
        assert_eq!(found.caches()[0].ecosystem, Ecosystem::Node(manager));
    }
}

#[test]
fn the_package_manager_field_decides_the_yarn_line() {
    let berry = workspace(&[
        (
            "package.json",
            r#"{"packageManager":"yarn@4.5.0+sha512.abc"}"#,
        ),
        ("yarn.lock", ""),
        (".yarnrc.yml", "nodeLinker: node-modules\n"),
    ])
    .unwrap();
    let found = detect(berry.path(), &[]).unwrap();
    assert_eq!(commands(&found), ["yarn install --immutable"]);
    assert_eq!(
        found.spec().tools,
        [tool("yarn", "4.5.0", ToolSource::Detected).unwrap()]
    );

    let classic = workspace(&[
        ("package.json", r#"{"packageManager":"yarn@1.22.22"}"#),
        ("yarn.lock", ""),
        (".yarnrc.yml", ""),
    ])
    .unwrap();
    assert_eq!(
        commands(&detect(classic.path(), &[]).unwrap()),
        ["yarn install --frozen-lockfile"]
    );
}

/// Plug'n'Play, Yarn's default, writes no `node_modules`; only the
/// `node-modules` linker gives a tree materialize can prove whole.
#[test]
fn yarn_berry_needs_the_node_modules_linker() {
    for (yarnrc, refused) in [
        ("", true),
        ("nodeLinker: pnp\n", true),
        ("nodeLinker: pnpm\n", true),
        ("  nodeLinker: node-modules\n", true),
        ("nodeLinker: node-modules\n", false),
        (
            "enableGlobalCache: true\nnodeLinker: \"node-modules\" # for Fabric\n",
            false,
        ),
        ("nodeLinker: 'node-modules'\n", false),
    ] {
        let dir = workspace(&[
            ("package.json", r#"{"packageManager":"yarn@4.5.0"}"#),
            ("yarn.lock", ""),
            (".yarnrc.yml", yarnrc),
        ])
        .unwrap();
        let found = detect(dir.path(), &[]);
        assert_eq!(
            matches!(found, Err(DetectError::YarnPlugAndPlay)),
            refused,
            "{yarnrc:?} {found:?}"
        );
    }
}

#[test]
fn two_lockfiles_with_nothing_to_decide_are_refused() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("pnpm-lock.yaml", ""),
        ("package-lock.json", ""),
    ])
    .unwrap();
    match detect(dir.path(), &[]) {
        Err(DetectError::AmbiguousLockfiles { lockfiles }) => {
            assert_eq!(lockfiles, ["pnpm-lock.yaml", "package-lock.json"]);
        }
        other => panic!("{other:?}"),
    }
}

/// A frozen install of a manager whose lockfile is absent fails on the
/// target; detection says so on the source instead.
#[test]
fn a_named_manager_without_its_lockfile_is_refused() {
    let dir = workspace(&[
        ("package.json", r#"{"packageManager":"pnpm@11.23.0"}"#),
        ("package-lock.json", ""),
    ])
    .unwrap();
    match detect(dir.path(), &[]) {
        Err(err @ DetectError::LockfileMissing { .. }) => {
            assert_eq!(
                err.to_string(),
                "pnpm was chosen by `packageManager` in package.json, but the lockfiles found are: package-lock.json; add its lockfile or set `package_manager` in .fabric/env.toml"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// With no lockfile nothing can rebuild `node_modules`, so it is not a
/// cache: it travels as the user's own files.
#[test]
fn package_json_without_a_lockfile_has_no_install_and_no_cache() {
    let dir = workspace(&[("package.json", r#"{"packageManager":"pnpm@11.23.0"}"#)]).unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    assert!(found.spec().setup.is_empty());
    assert!(found.caches().is_empty());
    assert_eq!(found.node_manager(), None);
}

#[test]
fn a_malformed_package_manager_field_is_refused() {
    for text in [
        r#"{"packageManager":"pnpm"}"#,
        r#"{"packageManager":"deno@2.0.0"}"#,
        r#"{"packageManager":"pnpm@latest"}"#,
        r#"{"packageManager":7}"#,
    ] {
        let dir = workspace(&[("package.json", text), ("pnpm-lock.yaml", "")]).unwrap();
        assert!(
            matches!(
                detect(dir.path(), &[]),
                Err(DetectError::BadPackageManagerField)
            ),
            "{text}"
        );
    }
}

/// Precedence, highest first: the override file, `mise.toml`,
/// `.tool-versions`, the tool's own file, then `package.json` engines.
#[test]
fn pins_follow_precedence_and_aliases() {
    let dir = workspace(&[
        (
            "mise.toml",
            "[tools]\nnode = \"24\"\npython = [\"3.12\", \"3.11\"]\nrust = { version = \"1.97.1\" }\n",
        ),
        (
            ".tool-versions",
            "# pins\nnodejs 22.1.0\ngolang 1.23.0 # fallback below\nterraform 1.9.0 1.8.0\n",
        ),
        ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.96.0\"\n"),
        (".node-version", "21\n"),
        (".nvmrc", "# lts\n\nv20\n"),
        (".python-version", "3.13\n"),
        ("go.mod", "module example.com/x\n\ngo 1.22.0\n"),
        ("package.json", r#"{"engines":{"node":">=18"}}"#),
    ])
    .unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    let pins: Vec<(&str, &str, &str)> = found
        .pins()
        .iter()
        .map(|p| (p.tool.as_str(), p.version.as_str(), p.file))
        .collect();
    assert_eq!(
        pins,
        [
            ("node", "24", "mise.toml"),
            ("python", "3.12", "mise.toml"),
            ("rust", "1.97.1", "mise.toml"),
            ("node", "22.1.0", ".tool-versions"),
            ("go", "1.23.0", ".tool-versions"),
            ("terraform", "1.9.0", ".tool-versions"),
            ("rust", "1.96.0", "rust-toolchain.toml"),
            ("node", "21", ".node-version"),
            ("node", "v20", ".nvmrc"),
            ("python", "3.13", ".python-version"),
            ("go", "1.22.0", "go.mod"),
            ("node", ">=18", "package.json"),
        ]
    );
    assert_eq!(
        found.spec().tools,
        [
            tool("go", "1.23.0", ToolSource::Detected).unwrap(),
            tool("node", "24", ToolSource::Detected).unwrap(),
            tool("python", "3.12", ToolSource::Detected).unwrap(),
            tool("rust", "1.97.1", ToolSource::Detected).unwrap(),
            tool("terraform", "1.9.0", ToolSource::Detected).unwrap(),
        ]
    );
    assert_eq!(commands(&found), ["go mod download"]);
    assert!(found.caches().is_empty());
}

#[test]
fn python_installs_into_the_workspace_venv() {
    let locked = workspace(&[("uv.lock", ""), ("requirements.txt", "")]).unwrap();
    let found = detect(locked.path(), &[]).unwrap();
    assert_eq!(commands(&found), ["uv sync --frozen"]);
    assert_eq!(
        found.caches(),
        [cache(
            ".venv",
            CacheClass::Deps,
            Ecosystem::Python,
            Completion::Sentinel { filled_by: vec![0] }
        )
        .unwrap()]
    );

    let requirements = workspace(&[("requirements.txt", "requests==2.32.3\n")]).unwrap();
    assert_eq!(
        commands(&detect(requirements.path(), &[]).unwrap()),
        [
            "uv venv --allow-existing .venv",
            "uv pip install --python .venv -r requirements.txt"
        ]
    );
}

#[test]
fn a_devcontainer_is_recorded_and_not_interpreted() {
    let dir = workspace(&[(
        ".devcontainer/devcontainer.json",
        r#"{"image":"x","postCreateCommand":"npm install"}"#,
    )])
    .unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    assert_eq!(
        found.devcontainer(),
        Some(&RelPath::parse(".devcontainer/devcontainer.json").unwrap())
    );
    assert!(found.spec().setup.is_empty());
}

/// The fixture has both lockfiles and pins Node in `.nvmrc`; the override
/// file decides the manager, replaces the steps and the pin, and adds a
/// cache, two variable names and a secret pattern.
#[test]
fn the_override_file_wins_over_detection() {
    let found = detect(&fixture("override"), &[]).unwrap();
    assert_eq!(
        found.node_manager(),
        Some(ManagerChoice {
            manager: NodeManager::Pnpm,
            because: ManagerSource::OverrideFile
        })
    );
    assert_eq!(
        commands(&found),
        ["pnpm install --frozen-lockfile", "pnpm run codegen"]
    );
    assert_eq!(found.spec().setup[1].when, RunWhen::Always);
    assert_eq!(
        found.spec().tools,
        [tool("node", "22.23.2", ToolSource::Declared).unwrap()]
    );
    assert_eq!(found.pins()[0].file, ".fabric/env.toml");
    assert_eq!(found.pins()[0].source, ToolSource::Declared);
    assert_eq!(found.pins()[1].version.as_str(), "20");
    assert_eq!(found.pins()[1].source, ToolSource::Detected);
    let names: Vec<&str> = found
        .spec()
        .env_var_names
        .iter()
        .map(EnvVarName::as_str)
        .collect();
    assert_eq!(names, ["DATABASE_URL", "OPENAI_API_KEY"]);
    assert_eq!(
        found.caches(),
        [
            CachePath {
                origin: CacheOrigin::Declared,
                ..cache(
                    "dist",
                    CacheClass::Build,
                    Ecosystem::Node(NodeManager::Pnpm),
                    Completion::Sentinel {
                        filled_by: vec![0, 1]
                    }
                )
                .unwrap()
            },
            cache(
                "node_modules",
                CacheClass::Deps,
                Ecosystem::Node(NodeManager::Pnpm),
                Completion::Sentinel {
                    filled_by: vec![0, 1]
                }
            )
            .unwrap(),
        ]
    );
    let local_db = RelPath::parse("data/local.sqlite").unwrap();
    assert_eq!(
        SecretPolicy::from_patterns(&found.spec().secret_patterns).denies(&local_db, Scope::Work),
        Some("*.sqlite")
    );
    assert!(found.spec().id().is_ok());
}

/// A declared pin under an alias still replaces the detected one, so the
/// spec never holds both `node` and `nodejs`; an empty tool name is refused.
#[test]
fn a_declared_pin_under_an_alias_replaces_the_detected_one() {
    let dir = workspace(&[
        (".nvmrc", "20\n"),
        (".fabric/env.toml", "[tools]\nnodejs = \"22\"\n"),
    ])
    .unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    assert_eq!(
        found.spec().tools,
        [tool("node", "22", ToolSource::Declared).unwrap()]
    );
    let dir = workspace(&[(".fabric/env.toml", "[tools]\n\"\" = \"22\"\n")]).unwrap();
    assert!(matches!(
        detect(dir.path(), &[]),
        Err(DetectError::Override(OverrideError::ToolName {
            index: 0,
            reason: PinError::Empty
        }))
    ));
}

/// The ecosystem decides the carry key; a declared class must not turn a
/// `node_modules` into something that travels to a node with another Node,
/// and on a case-folding volume `Node_Modules` is that same directory.
#[test]
fn a_declared_class_keeps_the_detected_ecosystem() {
    for declared in ["node_modules", "Node_Modules"] {
        let dir = workspace(&[
            ("package.json", PACKAGE_JSON),
            ("package-lock.json", ""),
            (
                ".fabric/env.toml",
                &format!("[[cache]]\npath = \"{declared}\"\nclass = \"tool-cache\"\n"),
            ),
        ])
        .unwrap();
        let found = detect(dir.path(), &[]).unwrap();
        assert_eq!(
            found.caches(),
            [cache(
                "node_modules",
                CacheClass::ToolCache,
                Ecosystem::Node(NodeManager::Npm),
                Completion::Sentinel { filled_by: vec![0] }
            )
            .unwrap()],
            "{declared}"
        );
    }
}

/// `node_modules/.cache` is inside a cache detection owns; declaring it
/// would give one tree two carry rules.
#[test]
fn a_declared_cache_inside_a_detected_one_is_refused() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        (
            ".fabric/env.toml",
            "[[cache]]\npath = \"node_modules/.cache\"\nclass = \"build\"\n",
        ),
    ])
    .unwrap();
    match detect(dir.path(), &[]) {
        Err(DetectError::NestedCache { index, detected }) => {
            assert_eq!(index, 0);
            assert_eq!(detected.as_str(), "node_modules");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_broken_override_file_stops_detection() {
    let dir = workspace(&[(".fabric/env.toml", "env = [\"A=b\"]\n")]).unwrap();
    assert!(matches!(
        detect(dir.path(), &[]),
        Err(DetectError::Override(_))
    ));
}

/// A `package_manager` with nothing to apply to is a mistake, not a no-op.
#[test]
fn a_package_manager_without_a_package_json_is_refused() {
    let dir = workspace(&[(".fabric/env.toml", "package_manager = \"pnpm\"\n")]).unwrap();
    assert!(matches!(
        detect(dir.path(), &[]),
        Err(DetectError::PackageManagerWithoutPackageJson)
    ));
}

/// Detection follows no link: `.nvmrc -> .env` would read the secret file
/// and put its first line into the spec as the Node version.
#[test]
fn a_linked_pin_file_is_refused_without_reading_it() {
    let secret = "sk-proj-abcdefghijklmnopqrstuvwxyz";
    let dir = workspace(&[(".env", &format!("OPENAI_API_KEY={secret}\n"))]).unwrap();
    symlink(dir.path().join(".env"), dir.path().join(".nvmrc")).unwrap();
    match detect(dir.path(), &[]) {
        Err(DetectError::NotARegularFile { file }) => assert_eq!(file, ".nvmrc"),
        other => panic!("{other:?}"),
    }
}

/// A file detection only checks for selects the step that reads it, so a
/// link there out of the workspace would have `npm ci` read a lockfile
/// that differs from node to node, one to a file the secret policy denies
/// names a file capture leaves behind, and a directory there would select
/// an install that cannot run. Each is refused with the file named, as a
/// linked pin file is.
#[test]
fn a_linked_or_non_regular_file_detection_checks_for_is_refused() {
    let elsewhere = workspace(&[("lock.json", "{}")]).unwrap();
    let linked = workspace(&[("package.json", PACKAGE_JSON)]).unwrap();
    symlink(
        elsewhere.path().join("lock.json"),
        linked.path().join("package-lock.json"),
    )
    .unwrap();
    let denied = workspace(&[(".env", "django==5.2\n")]).unwrap();
    symlink(".env", denied.path().join("requirements.txt")).unwrap();
    let directory = workspace(&[("Cargo.toml/x", "")]).unwrap();
    for (dir, name) in [
        (&linked, "package-lock.json"),
        (&denied, "requirements.txt"),
        (&directory, "Cargo.toml"),
    ] {
        match detect(dir.path(), &[]) {
            Err(DetectError::NotARegularFile { file }) => assert_eq!(file, name),
            other => panic!("{name}: {other:?}"),
        }
    }
}

/// A link inside the workspace to a regular file there, as Django's
/// `requirements.txt -> requirements/base.txt`, travels as a link and
/// resolves the same on every node, so it counts as the file it names:
/// the same steps, and the same install inputs, as a copy of that file.
#[test]
fn a_file_detection_checks_for_may_link_to_a_file_inside_the_workspace() {
    let base = "django==5.2\n";
    let copied = workspace(&[("requirements.txt", base)]).unwrap();
    let linked = workspace(&[("requirements/base.txt", base)]).unwrap();
    symlink(
        "requirements/base.txt",
        linked.path().join("requirements.txt"),
    )
    .unwrap();
    let copied = detect(copied.path(), &[]).unwrap();
    let linked = detect(linked.path(), &[]).unwrap();
    assert_eq!(commands(&linked), commands(&copied));
    assert_eq!(linked.inputs_digest(), copied.inputs_digest());
}

/// A link anywhere on the way is refused, not only in the last component:
/// `.fabric -> ../other/.fabric` would read another workspace's setup steps.
#[test]
fn a_linked_override_directory_is_refused() {
    let other = workspace(&[(
        ".fabric/env.toml",
        "[[setup]]\nprogram = \"sh\"\nargs = [\"-c\", \"echo other\"]\n",
    )])
    .unwrap();
    let dir = workspace(&[]).unwrap();
    symlink(other.path().join(".fabric"), dir.path().join(".fabric")).unwrap();
    match detect(dir.path(), &[]) {
        Err(DetectError::NotARegularFile { file }) => assert_eq!(file, ".fabric/env.toml"),
        other => panic!("{other:?}"),
    }
}

/// Pins are informational, so a pin a detected file gives that breaks the
/// pin rules is dropped and detection goes on: one row per rule. The other
/// pins stay, and the dropped text appears nowhere in the report.
#[test]
fn a_detected_pin_that_breaks_the_rules_is_dropped() {
    let token = format!("ghp_{}", "q7".repeat(18));
    let range = "^12.22.0 || ^14.17.0 || ^16.13.0 || ^18.12.0 || ^20.9.0 || >=21.1.0";
    let long_name = "cargo:https://github.com/some-organization/some-long-repository-name";
    let cases: [(&str, String, PinError); 5] = [
        (".nvmrc", token.clone(), PinError::Credential),
        (
            "package.json",
            format!(r#"{{"engines":{{"node":"{range}"}}}}"#),
            PinError::TooLong,
        ),
        (
            "mise.toml",
            format!("[tools]\n\"{long_name}\" = \"1\"\n"),
            PinError::TooLong,
        ),
        (
            ".tool-versions",
            "\u{1b}[31mnode 22\n".to_owned(),
            PinError::BadChar,
        ),
        (
            "mise.toml",
            "[tools]\nnode = \"\"\n".to_owned(),
            PinError::Empty,
        ),
    ];
    for (file, text, rule) in cases {
        let dir = workspace(&[(file, &text), (".python-version", "3.12\n")]).unwrap();
        let found = detect(dir.path(), &[]).unwrap();
        assert!(
            found.pins().iter().all(|pin| pin.file != file),
            "{file} {rule:?}"
        );
        assert_eq!(
            found.spec().tools,
            [tool("python", "3.12", ToolSource::Detected).unwrap()],
            "{file} {rule:?}"
        );
        let shown = format!("{found:?}");
        assert!(!shown.contains(&token) && !shown.contains(range) && !shown.contains(long_name));
    }
}

/// A declared path may hold the user's own files by mistake, so the full
/// secret list applies under it; the narrowed list stays for the caches an
/// ecosystem rule found.
#[test]
fn a_declared_cache_keeps_the_full_secret_list() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        (
            ".fabric/env.toml",
            "[[cache]]\npath = \"web\"\nclass = \"build\"\n",
        ),
    ])
    .unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    let scope_of = |path: &str| {
        found
            .caches()
            .iter()
            .find(|c| c.path.as_str() == path)
            .map(CachePath::secret_scope)
            .unwrap()
    };
    let leaked = RelPath::parse("web/.env.local").unwrap();
    assert_eq!(
        SecretPolicy::from_patterns(&found.spec().secret_patterns).denies(&leaked, scope_of("web")),
        Some(".env.*")
    );
    assert_eq!(scope_of("node_modules"), Scope::Cache);
}

/// The override file sits in the working tree, where the agent can rewrite
/// it, so a pattern the job once added stays in force after the file
/// drops it, and stays in the spec for the next detection.
#[test]
fn removing_a_pattern_from_the_override_file_does_not_shrink_the_policy() {
    let dir = workspace(&[(".fabric/env.toml", "secret_patterns = [\"*.sqlite\"]\n")]).unwrap();
    let first = detect(dir.path(), &[]).unwrap();
    fs::write(dir.path().join(".fabric/env.toml"), "").unwrap();
    let second = detect(dir.path(), &first.spec().secret_patterns).unwrap();
    let local_db = RelPath::parse("data/local.sqlite").unwrap();
    assert_eq!(
        SecretPolicy::from_patterns(&second.spec().secret_patterns).denies(&local_db, Scope::Work),
        Some("*.sqlite")
    );
    assert_eq!(second.spec().secret_patterns, first.spec().secret_patterns);
    assert_eq!(second.spec().secret_patterns.len(), 1);
}

/// Detection runs again on every resume with the job's last spec; the same
/// files must give the same spec, override file included.
#[test]
fn detecting_with_the_previous_spec_gives_the_same_spec() {
    let found = detect(&fixture("override"), &[]).unwrap();
    let again = detect(&fixture("override"), &found.spec().secret_patterns).unwrap();
    assert_eq!(again.spec(), found.spec());
    assert_eq!(again.spec().id().unwrap(), found.spec().id().unwrap());
    assert!(!found.spec().secret_patterns.is_empty());
}

/// ADR 0012 gives a build output the key of the ecosystem that made it, so
/// a declared cache takes the one ecosystem detection found, the one it
/// names, or `other` when nothing was found; it never guesses between two.
#[test]
fn a_declared_cache_takes_the_carry_key_of_the_ecosystem_that_makes_it() {
    let node_and_rust: Files = &[
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        ("Cargo.toml", ""),
    ];
    let cases: [(Files, &str, Result<Ecosystem, &str>); 6] = [
        (
            &[("package.json", PACKAGE_JSON), ("package-lock.json", "")],
            "",
            Ok(Ecosystem::Node(NodeManager::Npm)),
        ),
        (&[], "", Ok(Ecosystem::Other)),
        (node_and_rust, "", Err("ambiguous")),
        (node_and_rust, "ecosystem = \"rust\"\n", Ok(Ecosystem::Rust)),
        (
            &[("Cargo.toml", "")],
            "ecosystem = \"node\"\n",
            Err("not detected"),
        ),
        (
            &[("Cargo.toml", "")],
            "ecosystem = \"other\"\n",
            Ok(Ecosystem::Other),
        ),
    ];
    for (files, extra, expected) in cases {
        let override_toml = format!("[[cache]]\npath = \".next\"\nclass = \"build\"\n{extra}");
        let mut all = files.to_vec();
        all.push((".fabric/env.toml", &override_toml));
        let dir = workspace(&all).unwrap();
        let found = detect(dir.path(), &[]);
        match (expected, found) {
            (Ok(ecosystem), Ok(found)) => {
                let declared = found
                    .caches()
                    .iter()
                    .find(|c| c.path.as_str() == ".next")
                    .unwrap();
                assert_eq!(declared.ecosystem, ecosystem, "{files:?} {extra}");
                assert_eq!(declared.origin, CacheOrigin::Declared);
            }
            (Err("ambiguous"), Err(DetectError::AmbiguousCacheEcosystem { index: 0 }))
            | (Err("not detected"), Err(DetectError::CacheEcosystemNotDetected { index: 0 })) => {}
            (expected, found) => panic!("{files:?} {extra}: expected {expected:?}, got {found:?}"),
        }
    }
}

/// Naming another ecosystem for a path detection owns would give one tree two carry rules.
#[test]
fn a_declared_ecosystem_cannot_override_a_detected_one() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        (
            ".fabric/env.toml",
            "[[cache]]\npath = \"node_modules\"\nclass = \"deps\"\necosystem = \"other\"\n",
        ),
    ])
    .unwrap();
    assert!(matches!(
        detect(dir.path(), &[]),
        Err(DetectError::CacheEcosystemConflict { index: 0 })
    ));
}

/// A job that names a pin file as a secret keeps it out of the spec:
/// detection does not open it.
#[test]
fn a_file_the_job_denies_is_not_read() {
    let dir = workspace(&[
        ("mise.toml", "[tools]\nnode = \"24\"\n"),
        (".fabric/env.toml", "secret_patterns = [\"mise.toml\"]\n"),
    ])
    .unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    assert!(found.pins().is_empty());
    assert!(found.spec().tools.is_empty());
}

/// Capture leaves a denied lockfile behind, so the source must detect as
/// the target will, without it: no install for a lockfile that never
/// arrives, and no `--locked` for a `Cargo.lock` that stays home.
#[test]
fn a_lockfile_the_job_denies_counts_as_absent() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", "{}"),
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("Cargo.lock", ""),
        (
            ".fabric/env.toml",
            "secret_patterns = [\"package-lock.json\", \"cargo.lock\"]\n",
        ),
    ])
    .unwrap();
    let found = detect(dir.path(), &[]).unwrap();
    assert_eq!(commands(&found), ["cargo fetch"]);
    assert_eq!(found.node_manager(), None);
}
