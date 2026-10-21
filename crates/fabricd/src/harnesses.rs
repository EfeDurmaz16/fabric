//! Which adapter drives a harness on this node, and how it is configured.
//!
//! Invariant: the daemon and the CLI build an adapter the same way, from
//! the job's `HarnessKind`, the user's home, the `PATH` the user's shell
//! has, and the node's `harness.toml`, so a session captured by one
//! process is restored and launched by another under the same rules. No
//! adapter is ever given a per-job config directory (`CLAUDE_CONFIG_DIR`,
//! `CODEX_HOME`): a login is keyed to the config directory, and Fabric never
//! copies or proxies one (ADR 0006).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::harness::{HarnessKind, ProgramName};
use fabric_core::path::{AbsPath, PathError};
use fabric_harness::Harness;
use fabric_harness::acp::AcpHarness;
use fabric_harness::claude::ClaudeCode;
use fabric_harness::codex::Codex;
use serde::{Deserialize, Serialize};

/// Why a harness could not be named or configured.
#[derive(Debug, thiserror::Error)]
pub enum HarnessConfigError {
    /// The text is not `claude`, `codex` or `acp:<program> [args]`.
    #[error("unknown harness {0:?}; use claude, codex or acp:<program>")]
    Unknown(String),
    /// The ACP program name is not a bare program name.
    #[error("acp program: {0}")]
    Program(#[from] fabric_core::harness::ProgramNameError),
    /// The configuration file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The configuration file does not parse.
    #[error("{path}: {reason}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// What the parser said.
        reason: String,
    },
    /// The adapter refused its configuration.
    #[error(transparent)]
    Adapter(#[from] fabric_harness::HarnessError),
    /// The home or an ACP state directory is not UTF-8.
    #[error("{0} is not UTF-8")]
    NotUtf8(PathBuf),
    /// The home or an ACP state directory is not an absolute path without `.` or `..`.
    #[error("{path}: {reason}")]
    Path {
        /// The path.
        path: PathBuf,
        /// What is wrong with it.
        reason: PathError,
    },
}

fn abs_path(path: &Path) -> Result<AbsPath, HarnessConfigError> {
    let text = path
        .to_str()
        .ok_or_else(|| HarnessConfigError::NotUtf8(path.to_owned()))?;
    AbsPath::parse(text).map_err(|reason| HarnessConfigError::Path {
        path: path.to_owned(),
        reason,
    })
}

/// Parses the harness names the CLI takes: `claude`, `codex`, or `acp:<program> [args...]`.
pub fn parse_harness(text: &str) -> Result<HarnessKind, HarnessConfigError> {
    match text {
        "claude" | "claude-code" => Ok(HarnessKind::ClaudeCode),
        "codex" => Ok(HarnessKind::Codex),
        _ => {
            let spec = text
                .strip_prefix("acp:")
                .ok_or_else(|| HarnessConfigError::Unknown(text.to_owned()))?;
            let mut words = spec.split_whitespace();
            let program = words
                .next()
                .ok_or_else(|| HarnessConfigError::Unknown(text.to_owned()))?;
            let kind = HarnessKind::Acp {
                program: ProgramName::parse(program)?,
                args: words.map(str::to_owned).collect(),
            };
            refuse_secret_kind(&kind)?;
            Ok(kind)
        }
    }
}

/// Refuses a harness kind whose ACP arguments hold a secret shape.
///
/// The kind is written into ledger records that travel to every peer and
/// are never redacted, so the daemon checks every kind a request names
/// before it records one, not only at capture.
pub fn refuse_secret_kind(kind: &HarnessKind) -> Result<(), HarnessConfigError> {
    if let HarnessKind::Acp { program, args } = kind {
        fabric_harness::acp::refuse_secret_args(program, args)?;
    }
    Ok(())
}

/// One ACP agent's settings in `harness.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpConfig {
    /// Where the agent keeps its sessions; relative paths are under the home.
    pub state_dir: PathBuf,
}

/// The node's `harness.toml`: settings an adapter cannot discover.
///
/// The daemon reads it once at start and hands its copy to the CLI in
/// every run plan, so the harness a CLI launches is configured as the one
/// the daemon captures and restores, even when the file changed since.
///
/// ```toml
/// [acp.opencode]
/// state_dir = ".local/share/opencode/sessions"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfig {
    /// ACP agents by program name.
    #[serde(default)]
    pub acp: BTreeMap<String, AcpConfig>,
}

impl From<&HarnessConfig> for fabric_client::api::HarnessConfig {
    fn from(config: &HarnessConfig) -> Self {
        Self {
            acp: config
                .acp
                .iter()
                .map(|(program, acp)| {
                    let settings = fabric_client::api::AcpSettings {
                        state_dir: acp.state_dir.clone(),
                    };
                    (program.clone(), settings)
                })
                .collect(),
        }
    }
}

impl From<fabric_client::api::HarnessConfig> for HarnessConfig {
    fn from(config: fabric_client::api::HarnessConfig) -> Self {
        Self {
            acp: config
                .acp
                .into_iter()
                .map(|(program, settings)| {
                    let acp = AcpConfig {
                        state_dir: settings.state_dir,
                    };
                    (program, acp)
                })
                .collect(),
        }
    }
}

impl HarnessConfig {
    /// Reads `path`; a missing file is the empty configuration.
    pub fn load(path: &Path) -> Result<Self, HarnessConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(HarnessConfigError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|e| HarnessConfigError::Parse {
            path: path.to_owned(),
            reason: e.to_string(),
        })
    }
}

/// What every adapter on this node is built from.
#[derive(Debug, Clone)]
pub struct HarnessEnv {
    /// The user's home directory.
    pub home: AbsPath,
    /// The `PATH` to look programs up on.
    pub path_var: OsString,
    /// The node's harness configuration.
    pub config: HarnessConfig,
}

impl HarnessEnv {
    /// Builds the environment for `home`, which must be an absolute UTF-8 path.
    pub fn new(
        home: &Path,
        path_var: OsString,
        config: HarnessConfig,
    ) -> Result<Self, HarnessConfigError> {
        Ok(Self {
            home: abs_path(home)?,
            path_var,
            config,
        })
    }
}

/// Builds the adapter for `kind` on this node.
pub fn adapter(
    kind: &HarnessKind,
    env: &HarnessEnv,
) -> Result<Box<dyn Harness>, HarnessConfigError> {
    Ok(match kind {
        HarnessKind::ClaudeCode => {
            Box::new(ClaudeCode::new(env.home.clone(), env.path_var.clone()))
        }
        HarnessKind::Codex => Box::new(Codex::new(env.home.clone(), env.path_var.clone())),
        HarnessKind::Acp { program, args } => Box::new(acp_adapter(program, args, env)?),
    })
}

/// Builds the ACP adapter with the state directory `harness.toml` declares, if any.
pub fn acp_adapter(
    program: &ProgramName,
    args: &[String],
    env: &HarnessEnv,
) -> Result<AcpHarness, HarnessConfigError> {
    let adapter = AcpHarness::new(
        program.clone(),
        args.to_vec(),
        env.home.clone(),
        env.path_var.clone(),
    );
    match env.config.acp.get(program.as_str()) {
        Some(config) => {
            let dir = abs_path(&Path::new(env.home.as_str()).join(&config.state_dir))?;
            Ok(adapter.with_state_dir(&dir)?)
        }
        None => Ok(adapter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_names_parse_to_their_kinds() {
        assert_eq!(parse_harness("claude").unwrap(), HarnessKind::ClaudeCode);
        assert_eq!(parse_harness("codex").unwrap(), HarnessKind::Codex);
        assert_eq!(
            parse_harness("acp:opencode acp").unwrap(),
            HarnessKind::Acp {
                program: ProgramName::parse("opencode").unwrap(),
                args: vec!["acp".to_owned()],
            }
        );
        assert!(parse_harness("acp:/bin/sh").is_err());
        assert!(parse_harness("gemini").is_err());
    }

    #[test]
    fn an_unknown_key_in_harness_toml_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("harness.toml");
        std::fs::write(&path, "[acp.opencode]\nstate_dir = \"s\"\n").unwrap();
        assert_eq!(
            HarnessConfig::load(&path).unwrap().acp["opencode"].state_dir,
            PathBuf::from("s")
        );
        std::fs::write(&path, "[acp.opencode]\nstate = \"s\"\n").unwrap();
        assert!(HarnessConfig::load(&path).is_err());
    }
}
