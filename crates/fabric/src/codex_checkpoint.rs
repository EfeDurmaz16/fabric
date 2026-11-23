use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use fabric_core::path::RelPath;
use fabric_store::policy::IgnorePolicy;

const LIMIT: u64 = 64 * 1024 * 1024;

fn git(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| name == "PATH" || name == "HOME"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(root)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args);
    command
}

fn checked(command: &mut Command, action: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .with_context(|| format!("start {action}"))?;
    ensure!(
        output.status.success(),
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn paths(output: &[u8]) -> Result<BTreeSet<&str>> {
    Ok(std::str::from_utf8(output)
        .context("checkout paths must be UTF-8")?
        .split_terminator('\0')
        .collect())
}

fn validate_path(text: &str) -> Result<RelPath> {
    let rel = RelPath::parse(text)?;
    ensure!(
        !text
            .split('/')
            .any(|part| part.eq_ignore_ascii_case(".git")),
        "nested Git metadata at {text}"
    );
    Ok(rel)
}

/// A shallow checkout with fresh Git configuration and the source
/// index and working-tree bytes. Only the chosen checkout is staged for upload.
pub(crate) fn capture(root: &Path, scratch: &Path) -> Result<PathBuf> {
    let top = checked(
        &mut git(root, &["rev-parse", "--show-toplevel"]),
        "identify Git checkout",
    )?;
    ensure!(
        Path::new(std::str::from_utf8(&top)?.trim_end()).canonicalize()? == root,
        "--workspace must be the Git checkout root"
    );
    let mut policy = IgnorePolicy::default();
    policy.load_fabricignore(root)?;
    let tracked = checked(
        &mut git(root, &["ls-files", "-z", "--cached"]),
        "list index",
    )?;
    let head = checked(
        &mut git(root, &["ls-tree", "-r", "--name-only", "-z", "HEAD"]),
        "list HEAD",
    )?;
    let mut list_untracked = git(root, &["ls-files", "-z", "--others", "--exclude-standard"]);
    // Listing files is read-only. Honor the user's global Git ignore settings
    // here so an ignored local file cannot become eligible for upload.
    list_untracked.env_remove("GIT_CONFIG_GLOBAL");
    if let Some(config_dir) = std::env::var_os("XDG_CONFIG_HOME") {
        list_untracked.env("XDG_CONFIG_HOME", config_dir);
    }
    let untracked = checked(&mut list_untracked, "list untracked files")?;
    let index = checked(
        &mut git(root, &["ls-files", "-z", "--stage"]),
        "check index",
    )?;
    ensure!(
        !index
            .split(|byte| *byte == 0)
            .any(|row| row.starts_with(b"160000 ")),
        "submodules are unsupported for this handoff"
    );
    let flags = checked(
        &mut git(root, &["ls-files", "-z", "-v"]),
        "check index flags",
    )?;
    ensure!(
        !flags.split(|byte| *byte == 0).any(|row| row
            .first()
            .is_some_and(|tag| *tag == b'S' || tag.is_ascii_lowercase())),
        "sparse or assume-unchanged index entries are unsupported for this handoff"
    );
    check_intent_to_add(root)?;
    let tracked = paths(&tracked)?;
    for text in tracked.iter().chain(paths(&head)?.iter()) {
        ensure!(
            !policy.denies_anywhere(&validate_path(text)?),
            "tracked path excluded by Fabric policy: {text}"
        );
    }

    let stage = scratch.join("checkout");
    let patch = staged_patch(root)?;
    prepare_index(root, &stage, &patch)?;

    let mut size = 0_u64;
    for text in tracked.union(&paths(&untracked)?) {
        let rel = validate_path(text)?;
        if !policy.denies_anywhere(&rel) {
            size += copy_file(root, &stage, rel.as_str(), LIMIT - size)?;
        }
    }
    ensure!(
        staged_patch(root)? == patch,
        "the Git index changed during capture"
    );
    let archive = scratch.join("checkpoint.tar.gz");
    checked(
        Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg("."),
        "archive checkpoint",
    )?;
    ensure!(
        archive.metadata()?.len() <= LIMIT,
        "checkpoint archive exceeds 64 MiB"
    );
    Ok(archive)
}

fn check_intent_to_add(root: &Path) -> Result<()> {
    let list = |mode| {
        checked(
            &mut git(
                root,
                &[
                    "diff",
                    "--cached",
                    "--name-only",
                    "-z",
                    "--no-ext-diff",
                    mode,
                    "HEAD",
                ],
            ),
            "check intent-to-add entries",
        )
    };
    ensure!(
        list("--ita-visible-in-index")? == list("--ita-invisible-in-index")?,
        "intent-to-add index entries are unsupported for this handoff"
    );
    Ok(())
}

fn prepare_index(root: &Path, stage: &Path, patch: &[u8]) -> Result<()> {
    let mut clone = git(
        root,
        &[
            "clone",
            "--depth",
            "1",
            "--no-local",
            "--no-checkout",
            "--no-tags",
            "--template=",
            "--",
        ],
    );
    clone.arg(root).arg(stage);
    checked(&mut clone, "create shallow checkpoint repository")?;
    checked(
        &mut git(stage, &["read-tree", "HEAD"]),
        "initialize checkpoint index",
    )?;
    checked(
        &mut git(stage, &["remote", "remove", "origin"]),
        "remove source URL",
    )?;
    if !patch.is_empty() {
        let mut apply = git(
            stage,
            &["apply", "--cached", "--binary", "--whitespace=nowarn", "-"],
        )
        .stdin(Stdio::piped())
        .spawn()?;
        apply
            .stdin
            .take()
            .context("missing Git patch input")?
            .write_all(patch)?;
        ensure!(
            apply.wait()?.success(),
            "restore the staged index into the checkpoint"
        );
    }
    Ok(())
}

fn staged_patch(root: &Path) -> Result<Vec<u8>> {
    checked(
        &mut git(
            root,
            &[
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "HEAD",
            ],
        ),
        "capture staged changes",
    )
}

fn copy_file(root: &Path, stage: &Path, path: &str, remaining: u64) -> Result<u64> {
    let relative = Path::new(path);
    for parent in relative
        .ancestors()
        .skip(1)
        .filter(|part| !part.as_os_str().is_empty())
    {
        match fs::symlink_metadata(root.join(parent)) {
            Ok(info) => ensure!(
                info.is_dir(),
                "unsupported checkout parent: {}",
                parent.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        }
    }
    let from = root.join(path);
    let info = match fs::symlink_metadata(&from) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        info.is_file(),
        "only regular checkout files are supported: {path}"
    );
    ensure!(info.len() <= remaining, "checkpoint files exceed 64 MiB");
    let to = stage.join(path);
    fs::create_dir_all(to.parent().context("checkpoint path has no parent")?)?;
    fs::copy(&from, to)?;
    Ok(info.len())
}
