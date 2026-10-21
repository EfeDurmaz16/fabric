//! The environment a node runs tools in (ADR 0018, The client's environment).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use fabric_client::api::{ClientEnv, Roots};

/// The environment a node runs a tool in: the `PATH` it looks the tool up
/// on and runs it with, and each tool root, set or removed.
///
/// A request's [`ClientEnv`] becomes one, and so does the daemon's own
/// environment. That one never crosses the socket, so it keeps a `PATH`
/// entry or a root that is not UTF-8 as the system gave it, and a daemon
/// whose environment holds one starts as it always did (review finding on
/// PR #13): only what a client sends must be UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEnv {
    path: OsString,
    roots: [(&'static str, Option<OsString>); 4],
}

impl ToolEnv {
    /// The environment of this process, as [`ToolEnv::from_vars`] reads it.
    #[must_use]
    pub fn current() -> Self {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// The environment whose variables `var` reads: its `PATH` as it is,
    /// whose relative entries a lookup skips, and each root it sets to an
    /// absolute path, as [`ClientEnv::from_vars`] keeps them; a root it
    /// leaves unset or sets to a relative path is removed. Nothing here
    /// needs to be UTF-8.
    #[must_use]
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Self {
        Self {
            path: var("PATH").unwrap_or_default(),
            roots: Roots::default().vars().map(|(name, _)| {
                let value = var(name).filter(|value| value.as_bytes().first() == Some(&b'/'));
                (name, value)
            }),
        }
    }

    /// The `PATH` value to look a tool up on and run it with.
    #[must_use]
    pub fn path_var(&self) -> &OsStr {
        &self.path
    }

    /// Each root's variable and its value; `None` removes it.
    pub fn roots(&self) -> impl Iterator<Item = (&'static str, Option<&OsStr>)> {
        self.roots
            .iter()
            .map(|(name, value)| (*name, value.as_deref()))
    }

    /// The value of the root `name`, such as [`Roots::CARGO_HOME`], when set.
    #[must_use]
    pub fn root(&self, name: &str) -> Option<&OsStr> {
        self.roots()
            .find_map(|(var, value)| (var == name).then_some(value))
            .flatten()
    }
}

impl From<&ClientEnv> for ToolEnv {
    fn from(env: &ClientEnv) -> Self {
        Self {
            path: env.path_var(),
            roots: env
                .roots
                .vars()
                .map(|(name, value)| (name, value.map(|value| value.as_str().into()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's own environment may name a directory that is not UTF-8,
    /// which a client could not send; it is kept byte for byte, so the
    /// daemon still starts and runs tools from there.
    #[test]
    fn the_daemons_environment_keeps_what_is_not_utf8() {
        let path = OsStr::from_bytes(b"/usr/bin:/opt/caf\xe9/bin:/bin").to_owned();
        let cargo = OsStr::from_bytes(b"/home/\xff/.cargo").to_owned();
        let env = ToolEnv::from_vars(|name| match name {
            "PATH" => Some(path.clone()),
            Roots::CARGO_HOME => Some(cargo.clone()),
            Roots::PNPM_HOME => Some("relative/pnpm".into()),
            _ => None,
        });
        assert_eq!(env.path_var(), path);
        assert_eq!(env.root(Roots::CARGO_HOME), Some(cargo.as_os_str()));
        assert_eq!(env.root(Roots::PNPM_HOME), None);
        assert_eq!(env.roots().count(), 4);
    }
}
