use std::fs::{DirBuilder, File};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::codex_checkpoint;

pub(crate) struct Request<'a> {
    pub workspace: &'a Path,
    pub output: &'a Path,
    pub worker: &'a Path,
    pub project: Option<&'a Path>,
}

pub(crate) struct Vm {
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    worker: Child,
}

impl Vm {
    pub async fn launch(request: &Request<'_>) -> Result<(Self, PathBuf, Value)> {
        let root = request.workspace.canonicalize().context("open workspace")?;
        let parent = request
            .output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()
            .context("open the output parent directory")?;
        ensure!(
            !parent.starts_with(&root),
            "output directory must be outside the source checkout"
        );
        let stage = tempfile::tempdir().context("create private checkpoint staging directory")?;
        let checkpoint = codex_checkpoint::capture(&root, stage.path())?;
        let output = create_output(request.output)?;
        let version = codex_version().await?;
        let mut command = if request
            .worker
            .extension()
            .is_some_and(|ext| ext == "js" || ext == "mjs")
        {
            let path = request
                .worker
                .canonicalize()
                .context("locate Capsule worker")?;
            let mut command = Command::new("node");
            command.arg(path);
            command
        } else {
            Command::new(request.worker)
        };
        if let Some(project) = request.project {
            command.current_dir(project);
        }
        command
            .env_clear()
            .envs(std::env::vars_os().filter(|(name, _)| {
                name.to_str().is_some_and(|name| {
                    matches!(
                        name,
                        "PATH"
                            | "HOME"
                            | "TMPDIR"
                            | "VERCEL_TOKEN"
                            | "VERCEL_OIDC_TOKEN"
                            | "VERCEL_TEAM_ID"
                            | "VERCEL_PROJECT_ID"
                    )
                })
            }));
        command
            .args(["--profile", "vercel-handoff"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut worker = command
            .spawn()
            .context("start the Capsule Vercel handoff worker")?;
        let mut vm = Self {
            input: worker.stdin.take().context("missing Capsule stdin")?,
            output: BufReader::new(worker.stdout.take().context("missing Capsule stdout")?).lines(),
            worker,
        };
        vm.send(&serde_json::to_string(&json!({
            "action": "start",
            "version": version,
            "checkpoint": checkpoint,
            "output": output.join(".workspace.partial")
        }))?)
        .await?;
        let ready = vm.expect("ready").await?;
        Ok((vm, output, ready))
    }

    pub async fn collect(&mut self, output: &Path, answer: &str) -> Result<()> {
        self.send("collect").await?;
        self.expect("collected").await?;
        let partial = output.join(".workspace.partial");
        File::open(&partial)?.sync_all()?;
        std::fs::rename(&partial, output.join("workspace.tar.gz"))?;
        fabric_store::write_atomic(&output.join("answer.txt"), answer.as_bytes())?;
        File::open(output)?.sync_all()?;
        Ok(())
    }

    pub async fn delete(mut self) -> Result<()> {
        self.send("saved").await?;
        let deletion = self.expect("deleted").await?;
        ensure!(
            self.worker.wait().await?.success(),
            "Capsule cleanup exited unsuccessfully"
        );
        let name = deletion["name"]
            .as_str()
            .context("Capsule omitted the deleted sandbox name")?;
        writeln!(std::io::stderr().lock(), "Deleted Vercel sandbox {name}")?;
        Ok(())
    }

    async fn send(&mut self, message: &str) -> Result<()> {
        self.input.write_all(message.as_bytes()).await?;
        self.input.write_all(b"\n").await?;
        Ok(())
    }

    async fn expect(&mut self, state: &str) -> Result<Value> {
        let Some(line) = tokio::time::timeout(Duration::from_secs(150), self.output.next_line())
            .await
            .with_context(|| format!("Capsule did not reach {state}"))??
        else {
            bail!("Capsule stopped before {state}");
        };
        let reply: Value = serde_json::from_str(&line).context("invalid handoff worker reply")?;
        ensure!(reply["state"] == state, "expected Capsule state {state}");
        Ok(reply)
    }
}

fn create_output(path: &Path) -> Result<PathBuf> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .context("create a new output directory")?;
    Ok(path.canonicalize()?)
}

async fn codex_version() -> Result<String> {
    let output = Command::new("codex").arg("--version").output().await?;
    ensure!(
        output.status.success(),
        "cannot determine installed Codex version"
    );
    let printed = String::from_utf8(output.stdout)?;
    let version = printed
        .strip_prefix("codex-cli ")
        .context("unrecognized Codex version")?
        .trim();
    ensure!(
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "automated handoff needs a stable Codex release"
    );
    Ok(version.to_owned())
}
