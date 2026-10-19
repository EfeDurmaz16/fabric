//! Detection over fixture and scratch workspaces: what each file gives,
//! which source wins, and what the override file changes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fabric_core::env::{EnvVarName, RunWhen, Tool, ToolSource};
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::CacheClass;
use fabric_env::detect::{
    CachePath, DetectError, Detection, Ecosystem, ManagerChoice, ManagerSource, NodeManager, detect,
};
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
        .spec
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

fn tool(name: &str, version: &str, source: ToolSource) -> Tool {
    Tool {
        name: name.to_owned(),
        version: version.to_owned(),
        source,
    }
}

fn cache(path: &str, class: CacheClass, ecosystem: Ecosystem) -> Result<CachePath, PathError> {
    Ok(CachePath {
        path: RelPath::parse(path)?,
        class,
        ecosystem,
    })
}

const PACKAGE_JSON: &str = r#"{"name":"x","version":"1.0.0"}"#;

#[test]
fn a_pnpm_workspace_gives_a_frozen_install_and_node_modules() {
    let found = detect(&fixture("node-pnpm")).unwrap();
    assert_eq!(commands(&found), ["pnpm install --frozen-lockfile"]);
    assert_eq!(found.spec.setup[0].when, RunWhen::CachesMissing);
    assert_eq!(found.spec.setup[0].cwd, None);
    assert_eq!(
        found.caches,
        [cache(
            "node_modules",
            CacheClass::Deps,
            Ecosystem::Node(NodeManager::Pnpm)
        )
        .unwrap()]
    );
    assert_eq!(
        found.node_manager,
        Some(ManagerChoice {
            manager: NodeManager::Pnpm,
            because: ManagerSource::PackageManagerField
        })
    );
    assert_eq!(
        found.spec.tools,
        [
            tool("node", "22", ToolSource::Detected),
            tool("pnpm", "11.23.0", ToolSource::Detected)
        ]
    );
    assert_eq!(found.devcontainer, None);
    assert!(found.spec.id().is_ok());
}

/// A resume must not rewrite `Cargo.lock`; without one there is nothing to keep.
#[test]
fn cargo_fetch_is_locked_exactly_when_a_lockfile_exists() {
    let found = detect(&fixture("rust-cargo")).unwrap();
    assert_eq!(commands(&found), ["cargo fetch --locked"]);
    assert_eq!(
        found.caches,
        [cache("target", CacheClass::Build, Ecosystem::Rust).unwrap()]
    );
    assert_eq!(
        found.spec.tools,
        [tool("rust", "1.97.1", ToolSource::Detected)]
    );

    let library = workspace(&[("Cargo.toml", "[package]\nname = \"x\"\n")]).unwrap();
    assert_eq!(commands(&detect(library.path()).unwrap()), ["cargo fetch"]);
}

#[test]
fn each_lockfile_names_its_manager_and_install() {
    let cases: [(&[&str], NodeManager, &str); 6] = [
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
    ];
    for (files, manager, command) in cases {
        let mut all = vec![("package.json", PACKAGE_JSON)];
        all.extend(files.iter().map(|f| (*f, "")));
        let dir = workspace(&all).unwrap();
        let found = detect(dir.path()).unwrap();
        assert_eq!(commands(&found), [command], "{files:?}");
        assert_eq!(
            found.node_manager,
            Some(ManagerChoice {
                manager,
                because: ManagerSource::Lockfile
            }),
            "{files:?}"
        );
        assert_eq!(found.caches[0].ecosystem, Ecosystem::Node(manager));
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
    let found = detect(berry.path()).unwrap();
    assert_eq!(commands(&found), ["yarn install --immutable"]);
    assert_eq!(
        found.spec.tools,
        [tool("yarn", "4.5.0", ToolSource::Detected)]
    );

    let classic = workspace(&[
        ("package.json", r#"{"packageManager":"yarn@1.22.22"}"#),
        ("yarn.lock", ""),
        (".yarnrc.yml", ""),
    ])
    .unwrap();
    assert_eq!(
        commands(&detect(classic.path()).unwrap()),
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
    match detect(dir.path()) {
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
    match detect(dir.path()) {
        Err(DetectError::LockfileMissing {
            manager,
            because,
            found,
        }) => {
            assert_eq!(manager, NodeManager::Pnpm);
            assert_eq!(because, ManagerSource::PackageManagerField);
            assert_eq!(found, ["package-lock.json"]);
        }
        other => panic!("{other:?}"),
    }
}

/// With no lockfile nothing can rebuild `node_modules`, so it is not a
/// cache: it travels as the user's own files.
#[test]
fn package_json_without_a_lockfile_has_no_install_and_no_cache() {
    let dir = workspace(&[("package.json", r#"{"packageManager":"pnpm@11.23.0"}"#)]).unwrap();
    let found = detect(dir.path()).unwrap();
    assert!(found.spec.setup.is_empty());
    assert!(found.caches.is_empty());
    assert_eq!(found.node_manager, None);
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
            matches!(detect(dir.path()), Err(DetectError::BadPackageManagerField)),
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
    let found = detect(dir.path()).unwrap();
    let pins: Vec<(&str, &str, &str)> = found
        .pins
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
        found.spec.tools,
        [
            tool("go", "1.23.0", ToolSource::Detected),
            tool("node", "24", ToolSource::Detected),
            tool("python", "3.12", ToolSource::Detected),
            tool("rust", "1.97.1", ToolSource::Detected),
            tool("terraform", "1.9.0", ToolSource::Detected),
        ]
    );
    assert_eq!(commands(&found), ["go mod download"]);
    assert!(found.caches.is_empty());
}

#[test]
fn python_installs_into_the_workspace_venv() {
    let locked = workspace(&[("uv.lock", ""), ("requirements.txt", "")]).unwrap();
    let found = detect(locked.path()).unwrap();
    assert_eq!(commands(&found), ["uv sync --frozen"]);
    assert_eq!(
        found.caches,
        [cache(".venv", CacheClass::Deps, Ecosystem::Python).unwrap()]
    );

    let requirements = workspace(&[("requirements.txt", "requests==2.32.3\n")]).unwrap();
    assert_eq!(
        commands(&detect(requirements.path()).unwrap()),
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
    let found = detect(dir.path()).unwrap();
    assert_eq!(
        found.devcontainer,
        Some(RelPath::parse(".devcontainer/devcontainer.json").unwrap())
    );
    assert!(found.spec.setup.is_empty());
}

/// The fixture has both lockfiles and pins Node in `.nvmrc`; the override
/// file decides the manager, replaces the steps and the pin, and adds a
/// cache, two variable names and a secret pattern.
#[test]
fn the_override_file_wins_over_detection() {
    let found = detect(&fixture("override")).unwrap();
    assert_eq!(
        found.node_manager,
        Some(ManagerChoice {
            manager: NodeManager::Pnpm,
            because: ManagerSource::OverrideFile
        })
    );
    assert_eq!(
        commands(&found),
        ["pnpm install --frozen-lockfile", "pnpm run codegen"]
    );
    assert_eq!(found.spec.setup[1].when, RunWhen::Always);
    assert_eq!(
        found.spec.tools,
        [tool("node", "22.23.2", ToolSource::Declared)]
    );
    assert_eq!(found.pins[0].file, ".fabric/env.toml");
    assert_eq!(found.pins[1].version, "20");
    let names: Vec<&str> = found
        .spec
        .env_var_names
        .iter()
        .map(EnvVarName::as_str)
        .collect();
    assert_eq!(names, ["DATABASE_URL", "OPENAI_API_KEY"]);
    assert_eq!(
        found.caches,
        [
            cache("dist", CacheClass::Build, Ecosystem::Declared).unwrap(),
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
        found.secrets.denies(&local_db, Scope::Work),
        Some("*.sqlite")
    );
    assert!(found.spec.id().is_ok());
}

/// The ecosystem decides the carry key; a declared class must not turn a
/// `node_modules` into something that travels to a node with another Node.
#[test]
fn a_declared_class_keeps_the_detected_ecosystem() {
    let dir = workspace(&[
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        (
            ".fabric/env.toml",
            "[[cache]]\npath = \"node_modules\"\nclass = \"tool-cache\"\n",
        ),
    ])
    .unwrap();
    let found = detect(dir.path()).unwrap();
    assert_eq!(
        found.caches,
        [cache(
            "node_modules",
            CacheClass::ToolCache,
            Ecosystem::Node(NodeManager::Npm)
        )
        .unwrap()]
    );
}

#[test]
fn a_broken_override_file_stops_detection() {
    let dir = workspace(&[(".fabric/env.toml", "env = [\"A=b\"]\n")]).unwrap();
    assert!(matches!(detect(dir.path()), Err(DetectError::Override(_))));
}
