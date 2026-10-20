//! Detection over fixture and scratch workspaces: what each file gives,
//! which source wins, and what the override file changes.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use fabric_core::env::{EnvVarName, PinError, RunWhen, Tool, ToolName, ToolSource, ToolVersion};
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::CacheClass;
use fabric_env::detect::{
    CacheOrigin, CachePath, DetectError, Detection, Ecosystem, ManagerChoice, ManagerSource,
    NodeManager, detect,
};
use fabric_env::overrides::OverrideError;
use fabric_env::secret::Scope;
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
                .chain(step.args.iter().map(String::as_str))
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

fn cache(path: &str, class: CacheClass, ecosystem: Ecosystem) -> Result<CachePath, PathError> {
    Ok(CachePath {
        path: RelPath::parse(path)?,
        class,
        ecosystem,
        origin: CacheOrigin::Detected,
    })
}

const PACKAGE_JSON: &str = r#"{"name":"x","version":"1.0.0"}"#;

/// Files to write, as `(path, contents)`.
type Files<'a> = &'a [(&'a str, &'a str)];

#[test]
fn a_pnpm_workspace_gives_a_frozen_install_and_node_modules() {
    let found = detect(&fixture("node-pnpm"), None).unwrap();
    assert_eq!(commands(&found), ["pnpm install --frozen-lockfile"]);
    assert_eq!(found.spec().setup[0].when, RunWhen::CachesMissing);
    assert_eq!(found.spec().setup[0].cwd, None);
    assert_eq!(
        found.caches(),
        [cache(
            "node_modules",
            CacheClass::Deps,
            Ecosystem::Node(NodeManager::Pnpm)
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
    let found = detect(&fixture("rust-cargo"), None).unwrap();
    assert_eq!(commands(&found), ["cargo fetch --locked"]);
    assert_eq!(
        found.caches(),
        [cache("target", CacheClass::Build, Ecosystem::Rust).unwrap()]
    );
    assert_eq!(
        found.spec().tools,
        [tool("rust", "1.97.1", ToolSource::Detected).unwrap()]
    );

    let library = workspace(&[("Cargo.toml", "[package]\nname = \"x\"\n")]).unwrap();
    assert_eq!(
        commands(&detect(library.path(), None).unwrap()),
        ["cargo fetch"]
    );
}

#[test]
fn each_lockfile_names_its_manager_and_install() {
    let cases: [(&[&str], NodeManager, &str); 7] = [
        (
            &["pnpm-lock.yaml"],
            NodeManager::Pnpm,
            "pnpm install --frozen-lockfile",
        ),
        (&["package-lock.json"], NodeManager::Npm, "npm ci"),
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
        all.extend(files.iter().map(|f| (*f, "")));
        let dir = workspace(&all).unwrap();
        let found = detect(dir.path(), None).unwrap();
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
    ])
    .unwrap();
    let found = detect(berry.path(), None).unwrap();
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
        commands(&detect(classic.path(), None).unwrap()),
        ["yarn install --frozen-lockfile"]
    );
}

#[test]
fn two_lockfiles_with_nothing_to_decide_are_refused() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("pnpm-lock.yaml", ""),
        ("package-lock.json", ""),
    ])
    .unwrap();
    match detect(dir.path(), None) {
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
    match detect(dir.path(), None) {
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
    let found = detect(dir.path(), None).unwrap();
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
                detect(dir.path(), None),
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
    let found = detect(dir.path(), None).unwrap();
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
    let found = detect(locked.path(), None).unwrap();
    assert_eq!(commands(&found), ["uv sync --frozen"]);
    assert_eq!(
        found.caches(),
        [cache(".venv", CacheClass::Deps, Ecosystem::Python).unwrap()]
    );

    let requirements = workspace(&[("requirements.txt", "requests==2.32.3\n")]).unwrap();
    assert_eq!(
        commands(&detect(requirements.path(), None).unwrap()),
        [
            "uv venv --allow-existing",
            "uv pip install -r requirements.txt"
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
    let found = detect(dir.path(), None).unwrap();
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
    let found = detect(&fixture("override"), None).unwrap();
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
                    Ecosystem::Node(NodeManager::Pnpm)
                )
                .unwrap()
            },
            cache(
                "node_modules",
                CacheClass::Deps,
                Ecosystem::Node(NodeManager::Pnpm)
            )
            .unwrap(),
        ]
    );
    let local_db = RelPath::parse("data/local.sqlite").unwrap();
    assert_eq!(
        found.secrets().denies(&local_db, Scope::Work),
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
    let found = detect(dir.path(), None).unwrap();
    assert_eq!(
        found.spec().tools,
        [tool("node", "22", ToolSource::Declared).unwrap()]
    );
    let dir = workspace(&[(".fabric/env.toml", "[tools]\n\"\" = \"22\"\n")]).unwrap();
    assert!(matches!(
        detect(dir.path(), None),
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
        let found = detect(dir.path(), None).unwrap();
        assert_eq!(
            found.caches(),
            [cache(
                "node_modules",
                CacheClass::ToolCache,
                Ecosystem::Node(NodeManager::Npm)
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
    match detect(dir.path(), None) {
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
        detect(dir.path(), None),
        Err(DetectError::Override(_))
    ));
}

/// A `package_manager` with nothing to apply to is a mistake, not a no-op.
#[test]
fn a_package_manager_without_a_package_json_is_refused() {
    let dir = workspace(&[(".fabric/env.toml", "package_manager = \"pnpm\"\n")]).unwrap();
    assert!(matches!(
        detect(dir.path(), None),
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
    match detect(dir.path(), None) {
        Err(DetectError::NotARegularFile { file }) => assert_eq!(file, ".nvmrc"),
        other => panic!("{other:?}"),
    }
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
    match detect(dir.path(), None) {
        Err(DetectError::NotARegularFile { file }) => assert_eq!(file, ".fabric/env.toml"),
        other => panic!("{other:?}"),
    }
}

/// A pin travels in the spec like a setup argument does, so the same
/// credential guard applies, from a detected file and from the override file.
#[test]
fn a_credential_shaped_pin_is_refused_without_echoing_it() {
    let token = format!("ghp_{}", "q7".repeat(18));
    let detected = workspace(&[(".nvmrc", &token)]).unwrap();
    match detect(detected.path(), None) {
        Err(
            err @ DetectError::Pin {
                file,
                entry,
                reason,
            },
        ) => {
            assert_eq!((file, entry, reason), (".nvmrc", 0, PinError::Credential));
            assert!(!format!("{err} {err:?}").contains(&token));
        }
        other => panic!("{other:?}"),
    }
    let declared = workspace(&[(
        ".fabric/env.toml",
        &format!("[tools]\nnode = \"{token}\"\n"),
    )])
    .unwrap();
    match detect(declared.path(), None) {
        Err(err @ DetectError::Override(OverrideError::Pin { .. })) => {
            assert!(!format!("{err} {err:?}").contains(&token));
        }
        other => panic!("{other:?}"),
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
    let found = detect(dir.path(), None).unwrap();
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
        found.secrets().denies(&leaked, scope_of("web")),
        Some(".env.*")
    );
    assert_eq!(scope_of("node_modules"), Scope::Cache);
}

/// A tool name travels in the spec as a pin does: a token used as a name,
/// or a name that carries a terminal escape for the node that prints it,
/// is refused by its position and never quoted.
#[test]
fn a_bad_tool_name_is_refused_without_echoing_it() {
    let token = format!("ghp_{}", "q7".repeat(18));
    let declared = workspace(&[(
        ".fabric/env.toml",
        &format!("[tools]\n{token} = \"1\"\nnode = \"22\"\n"),
    )])
    .unwrap();
    match detect(declared.path(), None) {
        Err(err @ DetectError::Override(OverrideError::ToolName { .. })) => {
            assert!(!format!("{err} {err:?}").contains(&token));
            assert!(matches!(
                err,
                DetectError::Override(OverrideError::ToolName {
                    index: 0,
                    reason: PinError::Credential
                })
            ));
        }
        other => panic!("{other:?}"),
    }
    let escape = "\u{1b}[31mnode";
    for (file, text) in [
        (".tool-versions", format!("python 3.12\n{escape} 22\n")),
        (
            "mise.toml",
            "[tools]\n\"\\u001b[31mnode\\n\" = \"22\"\n".to_owned(),
        ),
    ] {
        let dir = workspace(&[(file, &text)]).unwrap();
        match detect(dir.path(), None) {
            Err(err @ DetectError::ToolName { .. }) => {
                assert!(!format!("{err}").contains('\u{1b}'), "{file}");
                assert!(
                    matches!(err, DetectError::ToolName { file: f, reason: PinError::BadChar, .. } if f == file),
                    "{file}"
                );
            }
            other => panic!("{file}: {other:?}"),
        }
    }
}

/// The override file sits in the working tree, where the agent can rewrite
/// it, so a pattern the job once added stays in force after the file
/// drops it, and stays in the spec for the next detection.
#[test]
fn removing_a_pattern_from_the_override_file_does_not_shrink_the_policy() {
    let dir = workspace(&[(".fabric/env.toml", "secret_patterns = [\"*.sqlite\"]\n")]).unwrap();
    let first = detect(dir.path(), None).unwrap();
    fs::write(dir.path().join(".fabric/env.toml"), "").unwrap();
    let second = detect(dir.path(), Some(first.spec())).unwrap();
    let local_db = RelPath::parse("data/local.sqlite").unwrap();
    assert_eq!(
        second.secrets().denies(&local_db, Scope::Work),
        Some("*.sqlite")
    );
    assert_eq!(second.spec().secret_patterns, first.spec().secret_patterns);
    assert_eq!(second.spec().secret_patterns.len(), 1);
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
        let found = detect(dir.path(), None);
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
        detect(dir.path(), None),
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
    let found = detect(dir.path(), None).unwrap();
    assert!(found.pins().is_empty());
    assert!(found.spec().tools.is_empty());
}
