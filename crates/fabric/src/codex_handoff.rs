//! One headless turn on an existing Codex thread with an authenticated remote executor.
//! The app server and its `ChatGPT` credentials remain on the local machine.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::codex_vm::{Request, Vm};

#[derive(Debug, Args)]
pub(crate) struct HandoffArgs {
    /// ID of an idle Codex thread saved on this machine.
    #[arg(long)]
    thread: String,
    /// WSS URL of a prepared Vercel Sandbox exec-server.
    #[arg(
        long,
        conflicts_with = "workspace",
        requires = "cwd",
        required_unless_present = "workspace"
    )]
    executor: Option<String>,
    /// Absolute workspace path inside the sandbox.
    #[arg(long, requires = "executor")]
    cwd: Option<PathBuf>,
    /// Idle local Git checkout to checkpoint and send to a new sandbox.
    #[arg(long, requires = "output", conflicts_with_all = ["executor", "cwd"])]
    workspace: Option<PathBuf>,
    /// New local directory for the returned checkout archive and answer.
    #[arg(long, requires = "workspace")]
    output: Option<PathBuf>,
    /// Capsule worker executable or built .js entry point.
    #[arg(long, default_value = "capsule-bridge-worker")]
    worker: PathBuf,
    /// Directory linked to the Vercel project for local SDK authentication.
    #[arg(long, requires = "workspace")]
    vercel_project: Option<PathBuf>,
    /// Prompt for the next turn. Pass the executor bearer on stdin.
    #[arg(long)]
    prompt: String,
    /// Override the thread's model with one available to the local account.
    #[arg(long)]
    model: Option<String>,
}

fn vercel_executor(endpoint: &str) -> bool {
    endpoint
        .strip_prefix("wss://")
        .and_then(|rest| rest.trim_end_matches('/').strip_suffix(".vercel.run"))
        .is_some_and(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(crate) async fn run(args: HandoffArgs) -> Result<ExitCode> {
    ensure!(!args.prompt.trim().is_empty(), "--prompt must not be empty");
    let answer = if let Some(workspace) = &args.workspace {
        managed(&args, workspace).await?
    } else {
        let executor = args.executor.as_deref().context("missing executor URL")?;
        let cwd = args
            .cwd
            .as_deref()
            .and_then(std::path::Path::to_str)
            .context("missing executor cwd")?;
        prepared(&args, executor, cwd).await?
    };
    writeln!(std::io::stdout().lock(), "{answer}")?;
    Ok(ExitCode::SUCCESS)
}

async fn managed(args: &HandoffArgs, workspace: &std::path::Path) -> Result<String> {
    let (mut vm, output, ready) = Vm::launch(&Request {
        workspace,
        output: args.output.as_deref().context("missing output directory")?,
        worker: &args.worker,
        project: args.vercel_project.as_deref(),
    })
    .await?;
    let executor = ready["url"]
        .as_str()
        .context("Capsule omitted the executor URL")?;
    let cwd = ready["cwd"]
        .as_str()
        .context("Capsule omitted the workspace path")?;
    let token = ready["token"]
        .as_str()
        .context("Capsule omitted the executor token")?;
    let turn = run_turn(args, executor, cwd, token).await;
    let answer = turn
        .as_deref()
        .unwrap_or("Codex turn failed. The archive contains the workspace at recovery.");
    vm.collect(&output, answer)
        .await
        .with_context(|| format!("recover output into {}", output.display()))?;
    vm.delete()
        .await
        .with_context(|| format!("output saved to {}", output.display()))?;
    turn.with_context(|| format!("recovered workspace saved to {}", output.display()))
}

async fn prepared(args: &HandoffArgs, executor: &str, cwd: &str) -> Result<String> {
    let mut bearer = String::new();
    tokio::io::stdin()
        .take(4097)
        .read_to_string(&mut bearer)
        .await?;
    let token = bearer.trim();
    ensure!(
        (32..=4096).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic()),
        "expected the executor capability token on stdin"
    );

    run_turn(args, executor, cwd, token).await
}

async fn run_turn(args: &HandoffArgs, executor: &str, cwd: &str, token: &str) -> Result<String> {
    ensure!(
        vercel_executor(executor),
        "expected wss://<sandbox>.vercel.run"
    );
    ensure!(
        std::path::Path::new(cwd).is_absolute(),
        "executor cwd must be absolute"
    );
    let (mut child, mut rpc) = start_app_server()?;
    rpc.call(
        "initialize",
        json!({
            "clientInfo": { "name": "fabric", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true }
        }),
    )
    .await?;
    rpc.send(json!({"jsonrpc": "2.0", "method": "initialized"}))
        .await?;

    let account = rpc
        .call("account/read", json!({"refreshToken": false}))
        .await?;
    ensure!(
        account["account"]["type"] == "chatgpt",
        "sign in to Codex with ChatGPT on this machine first"
    );

    let environment = "fabric-vercel";
    rpc.call(
        "environment/add",
        json!({
            "environmentId": environment,
            "execServerUrl": executor,
            "authBearerToken": token,
            "connectTimeoutMs": 10_000
        }),
    )
    .await?;
    let info = rpc
        .call("environment/info", json!({"environmentId": environment}))
        .await?;
    ensure!(
        info["cwd"].as_str().is_some(),
        "the remote executor did not report a working directory"
    );

    let mut resume = json!({
        "threadId": args.thread,
        "sandbox": "read-only",
        "approvalPolicy": "never",
        "config": {"mcp_servers": {}, "web_search": "disabled"}
    });
    if let Some(model) = &args.model {
        resume["model"] = model.as_str().into();
    }
    let resumed = rpc.call("thread/resume", resume).await?;
    ensure!(
        resumed["thread"]["id"] == args.thread,
        "Codex resumed a different thread"
    );

    let started = rpc
        .call(
            "turn/start",
            json!({
                "threadId": args.thread,
                "input": [{"type": "text", "text": args.prompt}],
                "environments": [{"environmentId": environment, "cwd": cwd}],
                "sandboxPolicy": {"type": "externalSandbox", "networkAccess": "restricted"}
            }),
        )
        .await?;
    let turn = started["turn"]["id"]
        .as_str()
        .context("Codex omitted the turn ID")?;
    let answer = tokio::time::timeout(Duration::from_mins(5), rpc.answer(&args.thread, turn))
        .await
        .context("Codex handoff turn timed out")??;
    drop(rpc);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    Ok(answer)
}

fn start_app_server() -> Result<(Child, Rpc)> {
    let mut child = Command::new("codex");
    child
        .args(["app-server", "--listen", "stdio://"])
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| {
            name.to_str().is_some_and(|name| {
                matches!(
                    name,
                    "HOME"
                        | "CODEX_HOME"
                        | "PATH"
                        | "TMPDIR"
                        | "USER"
                        | "LANG"
                        | "LOGNAME"
                        | "SHELL"
                )
            })
        }))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = child.spawn().context("start the local Codex app-server")?;
    let rpc = Rpc {
        input: child.stdin.take().context("missing app-server stdin")?,
        output: BufReader::new(child.stdout.take().context("missing app-server stdout")?).lines(),
        id: 0,
        early_turn_events: VecDeque::new(),
    };
    Ok((child, rpc))
}

struct Rpc {
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    id: u64,
    early_turn_events: VecDeque<Value>,
}

impl Rpc {
    async fn send(&mut self, message: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        self.input.write_all(&line).await?;
        Ok(())
    }

    async fn receive(&mut self) -> Result<Value> {
        let line = self
            .output
            .next_line()
            .await?
            .context("Codex app-server closed its output")?;
        let message: Value =
            serde_json::from_str(&line).context("decode Codex app-server message")?;
        if let Some(method) = message["method"].as_str()
            && !message["id"].is_null()
        {
            bail!("Codex requested an unsupported client action ({method})");
        }
        Ok(message)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.id += 1;
        let id = self.id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        let response = tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let message = self.receive().await?;
                if message["id"].as_u64() == Some(id) {
                    return Ok::<_, anyhow::Error>(message);
                }
                if method == "turn/start"
                    && matches!(
                        message["method"].as_str(),
                        Some("item/completed" | "turn/completed")
                    )
                {
                    self.early_turn_events.push_back(message);
                }
            }
        })
        .await
        .with_context(|| format!("Codex {method} timed out"))??;
        if let Some(error) = response.get("error") {
            // The provider's text may include request data such as the bearer token.
            bail!("Codex {method} failed (RPC error {})", error["code"]);
        }
        response
            .get("result")
            .cloned()
            .with_context(|| format!("missing Codex {method} result"))
    }

    async fn answer(&mut self, thread: &str, turn: &str) -> Result<String> {
        let mut answer = Vec::new();
        loop {
            let message = match self.early_turn_events.pop_front() {
                Some(message) => message,
                None => self.receive().await?,
            };
            let data = &message["params"];
            if data["threadId"] != thread {
                continue;
            }
            match message["method"].as_str() {
                Some("item/completed")
                    if data["turnId"] == turn && data["item"]["type"] == "agentMessage" =>
                {
                    if let Some(text) = data["item"]["text"].as_str() {
                        answer.push(text.to_owned());
                    }
                }
                Some("turn/completed") if data["turn"]["id"] == turn => {
                    ensure!(
                        data["turn"]["status"] == "completed",
                        "Codex turn ended with status {}",
                        data["turn"]["status"]
                    );
                    return Ok(answer.join("\n"));
                }
                _ => {}
            }
        }
    }
}
