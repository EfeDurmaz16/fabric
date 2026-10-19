//! Pre-seeding workspace trust in `~/.claude.json`.
//!
//! Invariant: the file is edited by read, modify, write to a temporary
//! file and rename, only the one key `projects["<root>"].hasTrustDialogAccepted`
//! changes, and the edit is refused while a `claude` process runs, because
//! Claude rewrites the file itself and keeps its own backups (ADR 0006,
//! Trust on the target; `permissions.md`, read 2026-10-19). A missing file
//! is not created: it means Claude never ran on this node, so there is no
//! login to trust a workspace for either.

use std::path::Path;

use serde_json::{Map, Value};

use crate::error::HarnessError;
use crate::files;

/// The key Claude reads to skip the trust dialog.
pub const TRUST_KEY: &str = "hasTrustDialogAccepted";

/// Sets the trust key for `root` in the config file at `config_json`.
/// Returns true when the file changed.
pub fn preseed_file(config_json: &Path, root: &Path) -> Result<bool, HarnessError> {
    let bytes = std::fs::read(config_json).map_err(|e| HarnessError::io("read", config_json, e))?;
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|e| HarnessError::Malformed {
        what: "config json",
        path: config_json.to_path_buf(),
        reason: e.to_string(),
    })?;
    let Some(top) = value.as_object_mut() else {
        return Err(HarnessError::Malformed {
            what: "config json",
            path: config_json.to_path_buf(),
            reason: "top level is not an object".to_owned(),
        });
    };
    let root_key = root.to_str().ok_or_else(|| HarnessError::Malformed {
        what: "workspace path",
        path: root.to_path_buf(),
        reason: "not UTF-8".to_owned(),
    })?;
    let projects = top
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else {
        return Err(HarnessError::Malformed {
            what: "config json",
            path: config_json.to_path_buf(),
            reason: "projects is not an object".to_owned(),
        });
    };
    let entry = projects
        .entry(root_key)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(entry) = entry.as_object_mut() else {
        return Err(HarnessError::Malformed {
            what: "config json",
            path: config_json.to_path_buf(),
            reason: format!("projects[{root_key:?}] is not an object"),
        });
    };
    if entry.get(TRUST_KEY) == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    entry.insert(TRUST_KEY.to_owned(), Value::Bool(true));
    let mut out = serde_json::to_vec_pretty(&value).map_err(|e| HarnessError::Malformed {
        what: "config json",
        path: config_json.to_path_buf(),
        reason: e.to_string(),
    })?;
    out.push(b'\n');
    files::write_atomic(config_json, &out)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_only_the_trust_key_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        std::fs::write(
            &config,
            r#"{"oauthAccount":{"emailAddress":"x"},"projects":{"/other":{"allowedTools":[]}}}"#,
        )
        .unwrap();
        assert!(preseed_file(&config, Path::new("/w")).unwrap());
        let value: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(value["projects"]["/w"][TRUST_KEY], Value::Bool(true));
        assert_eq!(
            value["projects"]["/other"]["allowedTools"],
            serde_json::json!([])
        );
        assert_eq!(value["oauthAccount"]["emailAddress"], "x");
        assert!(!preseed_file(&config, Path::new("/w")).unwrap());
    }

    #[test]
    fn a_missing_file_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        assert!(matches!(
            preseed_file(&config, Path::new("/w")),
            Err(HarnessError::Io { op: "read", .. })
        ));
        assert!(!config.exists());
    }
}
