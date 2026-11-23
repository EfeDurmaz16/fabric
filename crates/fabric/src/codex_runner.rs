//! Personal SSH runner for detached Codex handoffs.
use std::fs::{self, DirBuilder, File};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use fabric_core::{path::AbsPath, session::SessionId};
use fabric_harness::codex::Codex;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;

use crate::{codex_checkpoint, codex_handoff, codex_vm::Vm};

const FILE_LIMIT: u64 = 64 * 1024 * 1024;
const META_LIMIT: u64 = 65_536;
const MAX_ROLLOUTS: usize = 16;

#[derive(Debug, Subcommand)]
#[command(defer = true)]
pub(crate) enum RunnerCommand {
    /// Checkpoint and submit a local thread to a runner over SSH.
    Submit {
        /// SSH host or alias for the trusted Linux runner.
        #[arg(long)]
        host: String,
        /// Saved Codex thread ID on the submitting machine.
        #[arg(long)]
        thread: String,
        /// Git checkout root to checkpoint.
        #[arg(long)]
        workspace: PathBuf,
        /// Prompt for the next Codex turn.
        #[arg(long)]
        prompt: String,
        /// Optional model available to the runner account.
        #[arg(long)]
        model: Option<String>,
    },
    /// Receive a handoff from Fabric on stdin and durably acknowledge it.
    Accept {
        /// Private persistent queue directory.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Process the runner's persisted jobs independently of clients.
    Serve {
        /// Private persistent queue directory.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Capsule worker executable or compiled JavaScript entry point.
        #[arg(long, default_value = "capsule-bridge-worker")]
        worker: PathBuf,
        /// Vercel project directory on the runner.
        #[arg(long)]
        vercel_project: Option<PathBuf>,
        /// Process jobs already queued and then exit.
        #[arg(long)]
        once: bool,
    },
    /// Read a job's saved state on the runner.
    Status {
        /// Private persistent queue directory.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Job ID returned after a durable submission.
        job: String,
    },
}

pub(crate) async fn run(command: RunnerCommand, home: &Path) -> Result<ExitCode> {
    let default_root = || home.join(".local/share/fabric-codex-runner");
    match command {
        RunnerCommand::Submit {
            host,
            thread,
            workspace,
            prompt,
            model,
        } => {
            submit(home, &host, &thread, &workspace, &prompt, model).await?;
        }
        RunnerCommand::Accept { root } => {
            let root = root.unwrap_or_else(default_root);
            ensure!(cfg!(target_os = "linux"), "the Codex runner requires Linux");
            let id = accept(&root, &mut tokio::io::stdin()).await?;
            writeln!(std::io::stdout().lock(), "{id}")?;
        }
        RunnerCommand::Serve {
            root,
            worker,
            vercel_project,
            once,
        } => {
            ensure!(cfg!(target_os = "linux"), "the Codex runner requires Linux");
            let root = root.unwrap_or_else(default_root);
            serve(&root, home, &worker, vercel_project.as_deref(), once).await?;
        }
        RunnerCommand::Status { root, job } => {
            let root = root.unwrap_or_else(default_root);
            ensure!(valid_job_id(&job), "invalid runner job ID");
            let state = fs::read_to_string(root.join("jobs").join(job).join("state.json"))?;
            writeln!(std::io::stdout().lock(), "{state}")?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn valid_job_id(id: &str) -> bool {
    id.len() == 24
        && id
            .strip_prefix("job-")
            .is_some_and(|suffix| suffix.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn codex(home: &Path) -> Result<Codex> {
    let adapter = Codex::new(
        AbsPath::parse(home.to_str().context("home must be UTF-8")?)?,
        crate::path_var(),
    );
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        return Ok(adapter.with_codex_home(&AbsPath::parse(
            Path::new(&path)
                .to_str()
                .context("Codex home must be UTF-8")?,
        )?)?);
    }
    Ok(adapter)
}

async fn submit(
    home: &Path,
    host: &str,
    thread: &str,
    workspace: &Path,
    prompt: &str,
    model: Option<String>,
) -> Result<()> {
    ensure!(
        !host.is_empty()
            && !host.starts_with('-')
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.@".contains(&byte)),
        "expected an SSH alias or user@host"
    );
    ensure!(!prompt.trim().is_empty(), "prompt is empty");
    let scratch = tempfile::tempdir()?;
    let workspace = workspace.canonicalize()?;
    let checkpoint = codex_checkpoint::capture(&workspace, scratch.path())?;
    let rollout_dir = scratch.path().join("rollouts");
    fs::create_dir(&rollout_dir)?;
    let files = codex(home)?
        .portable_rollouts(&SessionId::parse(thread)?, &workspace, &rollout_dir)
        .await?;
    let names: Vec<_> = files
        .iter()
        .map(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .context("invalid rollout path")
        })
        .collect::<Result<_>>()?;
    let meta = json!({"thread": thread, "prompt": prompt, "model": model, "rollouts": names});
    validate(&meta)?;
    let mut child = Command::new("ssh")
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            host,
            "fabric codex-runner accept",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("connect to the trusted runner")?;
    let mut input = child.stdin.take().context("missing SSH input")?;
    let body = serde_json::to_vec(&meta)?;
    ensure!(
        body.len() as u64 <= META_LIMIT,
        "runner request metadata is too large"
    );
    input.write_u64(body.len() as u64).await?;
    input.write_all(&body).await?;
    send_file(&mut input, &checkpoint).await?;
    for file in files {
        send_file(&mut input, &file).await?;
    }
    drop(input);
    let response = child.wait_with_output().await?;
    ensure!(
        response.status.success(),
        "runner did not acknowledge the upload"
    );
    let id = std::str::from_utf8(&response.stdout)?.trim();
    ensure!(valid_job_id(id), "runner returned an invalid job ID");
    writeln!(std::io::stdout().lock(), "{id}")?;
    Ok(())
}

async fn send_file(output: &mut (impl AsyncWrite + Unpin), path: &Path) -> Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let length = file.metadata().await?.len();
    ensure!(
        (1..=FILE_LIMIT).contains(&length),
        "handoff file exceeds its size bound"
    );
    output.write_u64(length).await?;
    ensure!(
        tokio::io::copy(&mut (&mut file).take(length), output).await? == length,
        "handoff file changed"
    );
    Ok(())
}

fn validate(meta: &Value) -> Result<(&str, Vec<&str>)> {
    let thread = meta["thread"].as_str().context("missing thread ID")?;
    let id = SessionId::parse(thread)?;
    ensure!(
        meta["prompt"]
            .as_str()
            .is_some_and(|prompt| !prompt.trim().is_empty()),
        "missing prompt"
    );
    ensure!(
        meta["model"].is_null() || meta["model"].is_string(),
        "invalid model"
    );
    let entries = meta["rollouts"]
        .as_array()
        .context("missing rollout list")?;
    ensure!(
        (1..=MAX_ROLLOUTS).contains(&entries.len()),
        "invalid rollout count"
    );
    let mut names = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry.as_str().context("invalid rollout name")?;
        ensure!(
            !names.contains(&name) && Codex::rollout_thread(name).as_ref() == Some(&id),
            "rollout does not belong to the requested thread"
        );
        names.push(name);
    }
    Ok((thread, names))
}

async fn accept(root: &Path, input: &mut (impl AsyncRead + Unpin)) -> Result<String> {
    let count = input.read_u64().await?;
    ensure!(
        (1..=META_LIMIT).contains(&count),
        "runner request metadata is too large"
    );
    let mut body = Vec::new();
    ensure!(
        input.take(count).read_to_end(&mut body).await? as u64 == count,
        "incomplete request"
    );
    let meta: Value = serde_json::from_slice(&body)?;
    let (_, names) = validate(&meta)?;
    let jobs = prepare_root(root)?;
    let staged = tempfile::Builder::new()
        .prefix("job-")
        .rand_bytes(20)
        .tempdir_in(root)?;
    let stage = staged.path();
    let rollouts = stage.join("input-thread");
    fs::create_dir(&rollouts)?;
    recv_file(input, &stage.join("checkpoint.tar.gz")).await?;
    for name in names {
        recv_file(input, &rollouts.join(name)).await?;
    }
    ensure!(input.read(&mut [0]).await? == 0, "trailing request bytes");
    File::open(&rollouts)?.sync_all()?;
    fabric_store::write_atomic(&stage.join("request.json"), &body)?;
    state(stage, "queued", None)?;
    let id = stage
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid job staging path")?
        .to_owned();
    fs::rename(stage, jobs.join(&id))?;
    File::open(&jobs)?.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(id)
}

async fn recv_file(input: &mut (impl AsyncRead + Unpin), path: &Path) -> Result<()> {
    let length = input.read_u64().await?;
    ensure!((1..=FILE_LIMIT).contains(&length), "invalid file length");
    let mut file = tokio::fs::File::create_new(path).await?;
    ensure!(
        tokio::io::copy(&mut input.take(length), &mut file).await? == length,
        "incomplete uploaded file"
    );
    file.sync_all().await?;
    Ok(())
}

fn prepare_root(root: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(root.is_absolute(), "runner root must be absolute");
    DirBuilder::new().recursive(true).mode(0o700).create(root)?;
    let info = fs::symlink_metadata(root)?;
    ensure!(
        info.is_dir() && info.permissions().mode().trailing_zeros() >= 6,
        "runner root must be a private directory"
    );
    let jobs = root.join("jobs");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&jobs)?;
    ensure!(
        fs::symlink_metadata(&jobs)?.is_dir(),
        "invalid runner job directory"
    );
    Ok(jobs)
}

fn state(job: &Path, status: &str, detail: Option<&str>) -> Result<()> {
    let doc = json!({"status": status, "detail": detail, "job": job.file_name().and_then(std::ffi::OsStr::to_str), "result": job.join("result")});
    Ok(fabric_store::write_atomic(
        &job.join("state.json"),
        &serde_json::to_vec(&doc)?,
    )?)
}

async fn serve(
    root: &Path,
    home: &Path,
    worker: &Path,
    project: Option<&Path>,
    once: bool,
) -> Result<()> {
    let jobs = prepare_root(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("runner.lock"))?;
    lock.try_lock()
        .context("a worker already owns this queue")?;
    loop {
        let mut pending = fs::read_dir(&jobs)?.collect::<std::io::Result<Vec<_>>>()?;
        pending.sort_by_key(std::fs::DirEntry::file_name);
        for entry in pending {
            if !entry.file_type()?.is_dir() || !entry.file_name().to_str().is_some_and(valid_job_id)
            {
                continue;
            }
            let job = entry.path();
            let raw = fs::read(job.join("state.json"))?;
            let doc: Value = serde_json::from_slice(&raw)?;
            if doc["status"] == "running" || doc["status"] == "recovering" {
                if let Err(error) = recover(&job, home, worker, project).await {
                    state(&job, "recovering", Some(&format!("{error:#}")))?;
                }
            } else if doc["status"] == "queued" {
                state(&job, "running", None)?;
                if let Err(error) = execute(&job, home, worker, project).await {
                    let status = if job.join("sandbox-name").exists() {
                        "recovering"
                    } else {
                        "failed"
                    };
                    state(&job, status, Some(&format!("{error:#}")))?;
                }
            }
        }
        if once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn execute(job: &Path, home: &Path, worker: &Path, project: Option<&Path>) -> Result<()> {
    let meta: Value = serde_json::from_slice(&fs::read(job.join("request.json"))?)?;
    let (thread, names) = validate(&meta)?;
    import_rollouts(job, home, &names)?;
    let results = job.join("result");
    fs::create_dir(&results)?;
    let id = job
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .context("invalid job id")?;
    let encoded = id[4..]
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("");
    let name = format!("fabric-handoff-{encoded}");
    fabric_store::write_atomic(&job.join("sandbox-name"), name.as_bytes())?;
    let (mut vm, ready) = Vm::launch_checkpoint(
        &job.join("checkpoint.tar.gz"),
        &results,
        worker,
        project,
        Some(&name),
    )
    .await?;
    let field = |name: &str| {
        ready[name]
            .as_str()
            .with_context(|| format!("missing Capsule {name}"))
    };
    let turn = codex_handoff::run_turn(
        thread,
        meta["prompt"].as_str().context("missing prompt")?,
        meta["model"].as_str(),
        field("url")?,
        field("cwd")?,
        field("token")?,
        Some(&results),
    )
    .await;
    vm.collect(
        &results,
        turn.as_deref()
            .unwrap_or("Codex turn failed; workspace recovered."),
    )
    .await?;
    let files = save_thread(home, thread, &results).await?;
    if turn.is_ok() {
        ensure!(
            thread_advanced(job, &files)?,
            "Codex completed without saving an updated thread"
        );
    }
    let (status, detail) = match turn {
        Ok(_) => ("completed", None),
        Err(error) => ("failed", Some(format!("{error:#}"))),
    };
    save_outcome(job, status, detail.as_deref())?;
    vm.delete()
        .await
        .context("results saved, but VM deletion failed")?;
    state(job, status, detail.as_deref())
}

async fn recover(job: &Path, home: &Path, worker: &Path, project: Option<&Path>) -> Result<()> {
    let name = match fs::read_to_string(job.join("sandbox-name")) {
        Ok(name) => Some(name),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let results = job.join("result");
    fs::create_dir_all(&results)?;
    let saved = job.join("outcome.json");
    let cleanup_only = saved.exists();
    let mut recovery = None;
    if let Some(name) = name {
        recovery = Vm::recover(&name, &results, worker, project, cleanup_only).await?;
    }
    if !cleanup_only {
        let meta: Value = serde_json::from_slice(&fs::read(job.join("request.json"))?)?;
        let (thread, names) = validate(&meta)?;
        seed_missing_rollouts(job, home, &names)?;
        save_thread(home, thread, &results).await?;
        let checkpoint = job.join("checkpoint.tar.gz");
        let workspace = results.join("workspace.tar.gz");
        let original = fs::metadata(&checkpoint)?;
        let has_workspace = match fs::metadata(&workspace) {
            Ok(saved) => (saved.dev(), saved.ino()) != (original.dev(), original.ino()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::hard_link(&checkpoint, &workspace)?;
                false
            }
            Err(error) => return Err(error.into()),
        };
        let detail = if has_workspace {
            "Recovered the interrupted turn's sandbox workspace and saved Codex thread."
        } else {
            "Sandbox output unavailable; recovered the submitted Git checkpoint and saved Codex thread."
        };
        fabric_store::write_atomic(&results.join("answer.txt"), detail.as_bytes())?;
        File::open(&results)?.sync_all()?;
        save_outcome(job, "interrupted", Some(detail))?;
    }
    if let Some(vm) = recovery {
        vm.delete().await?;
    }
    let outcome: Value = serde_json::from_slice(&fs::read(saved)?)?;
    state(
        job,
        outcome["status"]
            .as_str()
            .context("invalid saved job status")?,
        outcome["detail"].as_str(),
    )
}

fn save_outcome(job: &Path, status: &str, detail: Option<&str>) -> Result<()> {
    fabric_store::write_atomic(
        &job.join("outcome.json"),
        &serde_json::to_vec(&json!({"status": status, "detail": detail}))?,
    )?;
    Ok(())
}

async fn save_thread(home: &Path, thread: &str, results: &Path) -> Result<Vec<PathBuf>> {
    let saved = results.join("thread");
    let staging = tempfile::tempdir_in(results)?;
    let files = codex(home)?
        .portable_rollouts(&SessionId::parse(thread)?, results, staging.path())
        .await?;
    ensure!(!files.is_empty(), "Codex export returned no rollouts");
    sync_directories(staging.path())?;
    let relative = files
        .iter()
        .map(|file| {
            file.strip_prefix(staging.path())
                .map(|part| saved.join(part))
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if saved.exists() {
        fs::remove_dir_all(&saved)?;
    }
    fs::rename(staging.path(), &saved)?;
    File::open(results)?.sync_all()?;
    Ok(relative)
}

fn rollout_directory(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map_or_else(|| home.join(".codex"), PathBuf::from)
        .join("sessions")
        .join("fabric-runner")
}

fn seed_missing_rollouts(job: &Path, home: &Path, names: &[&str]) -> Result<()> {
    let target = rollout_directory(home);
    fs::create_dir_all(&target)?;
    for name in names {
        let path = target.join(name);
        if !path.exists() {
            fabric_store::write_atomic(&path, &fs::read(job.join("input-thread").join(name))?)?;
        }
    }
    Ok(())
}

fn thread_advanced(job: &Path, files: &[PathBuf]) -> Result<bool> {
    for file in files {
        let name = file.file_name().context("invalid captured rollout path")?;
        let previous = fs::read(job.join("input-thread").join(name));
        match previous {
            Ok(before) if fs::read(file)? != before => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error.into()),
            _ => {}
        }
    }
    Ok(false)
}

fn import_rollouts(job: &Path, home: &Path, names: &[&str]) -> Result<()> {
    let target = rollout_directory(home);
    fs::create_dir_all(&target)?;
    for name in names {
        let incoming = fs::read(job.join("input-thread").join(name))?;
        let path = target.join(name);
        match fs::read(&path) {
            Ok(existing) => ensure!(
                existing == incoming,
                "runner already holds a different revision of this thread"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fabric_store::write_atomic(&path, &incoming)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn sync_directories(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directories(&entry.path())?;
        } else {
            File::open(entry.path())?.sync_all()?;
        }
    }
    File::open(root)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "assert fixture construction and test success"
)]
mod tests {
    use super::{accept, recover, save_outcome, state, valid_job_id};
    use serde_json::{Value, json};
    use std::fs;

    const THREAD: &str = "018e23e0-51ca-7a78-bce5-ffde3ddc1f8b";
    const ROLLOUT: &str = "rollout-2026-11-23T00-00-01-018e23e0-51ca-7a78-bce5-ffde3ddc1f8b.jsonl";

    fn request(rollout: &str) -> Vec<u8> {
        let doc = serde_json::to_vec(&json!({
            "thread": THREAD,
            "prompt": "Continue",
            "rollouts": [rollout]
        }))
        .unwrap();
        let mut request = Vec::new();
        for bytes in [doc.as_slice(), b"checkpoint", b"thread"] {
            request.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            request.extend_from_slice(bytes);
        }
        request
    }

    #[tokio::test]
    async fn acknowledgement_follows_durable_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runner");
        let data = request(ROLLOUT);
        let id = accept(&root, &mut data.as_slice()).await.unwrap();
        assert!(valid_job_id(&id));
        let job = root.join("jobs").join(&id);
        assert_eq!(
            fs::read(job.join("checkpoint.tar.gz")).unwrap(),
            b"checkpoint"
        );
        assert_eq!(
            fs::read(job.join("input-thread").join(ROLLOUT)).unwrap(),
            b"thread"
        );
        let value: Value =
            serde_json::from_slice(&fs::read(job.join("state.json")).unwrap()).unwrap();
        assert_eq!(value["status"], "queued");
        state(&job, "completed", None).unwrap();
        let value: Value =
            serde_json::from_slice(&fs::read(job.join("state.json")).unwrap()).unwrap();
        assert_eq!(value["status"], "completed");
    }

    #[tokio::test]
    async fn finalized_recovery_does_not_require_a_worker_or_a_model_turn() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runner");
        let id = accept(&root, &mut request(ROLLOUT).as_slice())
            .await
            .unwrap();
        let job = root.join("jobs").join(id);
        state(&job, "recovering", None).unwrap();
        save_outcome(&job, "interrupted", Some("saved partial output")).unwrap();
        let nonexistent_worker = temp.path().join("must-not-start");
        for _ in 0..2 {
            recover(&job, temp.path(), &nonexistent_worker, None)
                .await
                .unwrap();
        }
        let doc: Value =
            serde_json::from_slice(&fs::read(job.join("state.json")).unwrap()).unwrap();
        assert_eq!(doc["status"], "interrupted");
        assert_eq!(doc["detail"], "saved partial output");
    }

    #[tokio::test]
    async fn partial_upload_never_queues_a_job() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runner");
        let mut data = request(ROLLOUT);
        data.pop();
        assert!(accept(&root, &mut data.as_slice()).await.is_err());
        assert_eq!(fs::read_dir(root.join("jobs")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn rejects_non_rollout_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runner");
        let data = request("../auth.json");
        assert!(accept(&root, &mut data.as_slice()).await.is_err());
        assert!(!root.join("jobs").exists());
    }
}
