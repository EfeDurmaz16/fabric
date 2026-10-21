//! The fixture: two nodes on one machine, each a `fabric serve` with its
//! own state directory, sharing one home, as the two gate Macs share one
//! user and one home path (ADR 0011). Nodes talk over loopback on the wire
//! the test names. The harness is the fake ACP agent, which keeps one JSON
//! file per session under the home, so its state travels the way a real
//! harness's does. The repository fixture needs git and nothing else: no
//! pnpm and no native Node module, which the gate's second Mac lacks.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What every fixture step returns: a failure is the test failing, with its reason.
pub type Fallible<T = ()> = Result<T, Box<dyn Error>>;

/// How long one CLI command may take before the test fails.
const COMMAND_TIMEOUT: Duration = Duration::from_mins(3);

/// How long a daemon may take to answer after it starts.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a fault point may take to be reached.
const FAULT_TIMEOUT: Duration = Duration::from_mins(2);

/// What every process the test starts shares.
pub struct Env {
    _dir: tempfile::TempDir,
    /// The test's directory, resolved to its physical path.
    pub root: PathBuf,
    /// The home both nodes share.
    pub home: PathBuf,
    /// The repository the job is made from.
    pub repo: PathBuf,
    path: OsString,
}

impl Env {
    /// Makes the directory, the home and the repository.
    ///
    /// The directory sits under `/tmp` so the control socket's path stays
    /// within the 103 bytes a Unix socket path may have on macOS.
    pub fn new() -> Fallible<Arc<Self>> {
        let dir = tempfile::Builder::new().prefix("fe2e").tempdir_in("/tmp")?;
        let root = fs::canonicalize(dir.path())?;
        let home = root.join("home");
        let bin = root.join("bin");
        fs::create_dir_all(&home)?;
        fs::create_dir_all(&bin)?;
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_fabric-fake-agent"),
            bin.join("fabric-fake-agent"),
        )?;
        let mut dirs = vec![bin];
        if let Some(dir) = find("git").as_deref().and_then(Path::parent) {
            dirs.push(dir.to_owned());
        }
        dirs.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        let path = std::env::join_paths(dirs)?;
        let repo = root.join("repo");
        make_repo(&repo)?;
        Ok(Arc::new(Self {
            _dir: dir,
            root,
            home,
            repo,
            path,
        }))
    }

    /// The job's root, the same path on both nodes.
    pub fn work(&self, job: &str) -> PathBuf {
        self.home.join("fabric").join("jobs").join(job).join("work")
    }

    /// Puts a fake `claude` first on the `PATH`: it answers `--version` as
    /// the gate's Claude does and otherwise writes its arguments, one per
    /// line, to `fake_claude_args` and exits.
    pub fn install_fake_claude(&self) -> Fallible {
        let claude = self.root.join("bin").join("claude");
        fs::write(
            &claude,
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo '2.1.294 (Claude Code)'; exit 0; fi\n\
             printf '%s\\n' \"$@\" > \"$HOME/claude-args\"\n",
        )?;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    /// The arguments the fake `claude` was last launched with.
    pub fn fake_claude_args(&self) -> Fallible<Vec<String>> {
        Ok(fs::read_to_string(self.home.join("claude-args"))?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// The fake agent's file for its first session.
    pub fn session_file(&self) -> PathBuf {
        self.home.join(".fake-agent").join("fake-1.json")
    }

    /// The prompts the fake agent's session holds, in order.
    pub fn prompts(&self) -> Fallible<Vec<String>> {
        let state: serde_json::Value = serde_json::from_slice(&fs::read(self.session_file())?)?;
        let prompts = state["prompts"]
            .as_array()
            .ok_or("the session holds no prompt list")?;
        Ok(prompts
            .iter()
            .filter_map(|prompt| prompt["text"].as_str().map(str::to_owned))
            .collect())
    }

    fn command(&self, program: &str, state: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.path)
            .env("FABRIC_HOME", state)
            .env("FAKE_STATE_DIR", self.home.join(".fake-agent"))
            .stdin(Stdio::null());
        if let Some(level) = std::env::var_os("FABRIC_LOG") {
            command.env("FABRIC_LOG", level);
        }
        command
    }
}

fn find(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn git(dir: &Path, args: &[&str]) -> Fallible {
    let status = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=fabric",
            "-c",
            "user.email=fabric@example.com",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdout(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed").into())
    }
}

fn make_repo(repo: &Path) -> Fallible {
    fs::create_dir_all(repo.join("src"))?;
    fs::write(repo.join("README.md"), "hello\n")?;
    fs::write(repo.join("src/greet.txt"), "hello, world\n")?;
    fs::write(repo.join(".gitignore"), "scratch/\n")?;
    git(repo, &["init", "-q", "-b", "main"])?;
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-q", "-m", "first"])
}

/// A process named `claude` that is not the job's, as the Claude Code a
/// user keeps open: a copy of the fake agent under that name, waiting on
/// a standard input the test holds open, killed when dropped.
///
/// It is not a copy of `/bin/sleep`: macOS kills a copy of a system binary
/// run from another path with `SIGKILL`, at once or seconds later, which
/// made the probe miss it under load.
pub struct OtherClaude(Child);

impl OtherClaude {
    /// Starts it under the test's directory and returns once `pgrep -x
    /// claude`, the probe the trust pre-seed runs, lists it.
    pub fn start(env: &Env) -> Fallible<Self> {
        let dir = env.root.join("other");
        fs::create_dir_all(&dir)?;
        let claude = dir.join("claude");
        fs::copy(env!("CARGO_BIN_EXE_fabric-fake-agent"), &claude)?;
        let other = Self(
            Command::new(&claude)
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let pid = other.pid().to_string();
        let pgrep = ["/usr/bin/pgrep", "/bin/pgrep"]
            .into_iter()
            .find(|path| Path::new(path).is_file())
            .ok_or("no pgrep")?;
        let started = Instant::now();
        loop {
            let listed = Command::new(pgrep)
                .args(if cfg!(target_os = "macos") {
                    &["-a", "-x", "claude"][..]
                } else {
                    &["-x", "claude"][..]
                })
                .output()?;
            if String::from_utf8_lossy(&listed.stdout)
                .split_whitespace()
                .any(|listed| listed == pid)
            {
                return Ok(other);
            }
            if started.elapsed() > START_TIMEOUT {
                return Err(format!("pgrep never listed the other claude, pid {pid}").into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Its process id.
    pub fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for OtherClaude {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Which wire a node dials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// QUIC only.
    Quic,
    /// TCP with TLS only.
    TcpTls,
}

impl Wire {
    fn flag(self) -> &'static str {
        match self {
            Wire::Quic => "quic",
            Wire::TcpTls => "tcp-tls",
        }
    }
}

/// One node: a state directory, a port, and the daemon while it runs.
pub struct Node {
    env: Arc<Env>,
    /// The name the test gives it, for messages.
    pub name: &'static str,
    state: PathBuf,
    port: u16,
    wire: Wire,
    daemon: Option<Child>,
    /// Its node id, known once it has started.
    pub id: String,
}

/// How one try at starting a daemon ended.
enum Started {
    /// It answers and has caught up with its peers.
    Up,
    /// It exited because another process took its port first.
    PortTaken,
}

/// The next port to try, below the range macOS and Linux hand out for
/// ephemeral sockets, starting at a point that differs between test runs.
static NEXT_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// A port free for both UDP and TCP on loopback, and given to no other node
/// of this test run.
///
/// Each port is handed out once, so a node killed and started again finds
/// its port free: a port the system picked would be offered to the next
/// node another test starts while this one is down. Another process, such
/// as a run in a sibling worktree, can still bind it between this probe and
/// the daemon's start; the first start then moves to the next port.
fn free_port() -> Fallible<u16> {
    use std::sync::atomic::Ordering;
    let base = 20_000 + u16::try_from(std::process::id() % 10_000)?;
    let _ = NEXT_PORT.compare_exchange(0, base, Ordering::SeqCst, Ordering::SeqCst);
    loop {
        let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);
        if port >= 32_000 {
            return Err("no free port below the ephemeral range".into());
        }
        let udp = std::net::UdpSocket::bind(("127.0.0.1", port));
        let tcp = std::net::TcpListener::bind(("127.0.0.1", port));
        if udp.is_ok() && tcp.is_ok() {
            return Ok(port);
        }
    }
}

impl Node {
    /// A node that has not started yet.
    pub fn new(env: &Arc<Env>, name: &'static str, wire: Wire) -> Fallible<Self> {
        let state = env.root.join(name);
        fs::create_dir_all(&state)?;
        // The daemon's log is written here too, which the node does not
        // write itself, so the directory starts private as a user's would.
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
        fs::write(
            state.join("harness.toml"),
            "[acp.fabric-fake-agent]\nstate_dir = \".fake-agent\"\n",
        )?;
        Ok(Self {
            env: Arc::clone(env),
            name,
            state,
            port: free_port()?,
            wire,
            daemon: None,
            id: String::new(),
        })
    }

    /// Its state directory, the `FABRIC_HOME` its commands run with.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// The address it listens on.
    pub fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The file the daemon writes when it reaches the armed fault point.
    pub fn signal(&self) -> PathBuf {
        self.state.join("fault-reached")
    }

    /// Starts the daemon, with `fault` armed when given, and waits until it
    /// answers and has caught up with its peers.
    ///
    /// When another process took the port before the first start, the node
    /// takes the next free one. After that its peers know its address, so a
    /// restart keeps the port and a taken port fails the test.
    pub fn start(&mut self, fault: Option<&str>) -> Fallible {
        if self.daemon.is_some() {
            return Err(format!("{} already runs", self.name).into());
        }
        loop {
            match self.try_start(fault)? {
                Started::Up => return Ok(()),
                Started::PortTaken if self.id.is_empty() => self.port = free_port()?,
                Started::PortTaken => {
                    return Err(format!("{} lost its port:\n{}", self.name, self.log()).into());
                }
            }
        }
    }

    fn try_start(&mut self, fault: Option<&str>) -> Fallible<Started> {
        match fs::remove_file(self.signal()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let log = fs::File::create(self.state.join("daemon.log"))?;
        let mut command = self.env.command(env!("CARGO_BIN_EXE_fabric"), &self.state);
        command
            .args([
                "serve",
                "--listen",
                &self.addr(),
                "--wire",
                self.wire.flag(),
            ])
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some(fault) = fault {
            command
                .env("FABRIC_FAULT", fault)
                .env("FABRIC_FAULT_SIGNAL", self.signal());
        }
        self.daemon = Some(command.spawn()?);
        let started = Instant::now();
        loop {
            let output = self.run(&["node", "id"])?;
            if output.status.success() {
                String::from_utf8(output.stdout)?
                    .lines()
                    .next()
                    .ok_or("node id printed nothing")?
                    .clone_into(&mut self.id);
                break;
            }
            if let Some(daemon) = self.daemon.as_mut()
                && daemon.try_wait()?.is_some()
            {
                self.daemon = None;
                if self.log().contains("Address already in use") {
                    return Ok(Started::PortTaken);
                }
                return Err(format!("{} exited at start:\n{}", self.name, self.log()).into());
            }
            if started.elapsed() > START_TIMEOUT {
                return Err(format!("{} did not start:\n{}", self.name, self.log()).into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        while self.ok(&["job", "ls"])?.contains("still catching up") {
            if started.elapsed() > START_TIMEOUT {
                return Err(format!("{} did not recover:\n{}", self.name, self.log()).into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(Started::Up)
    }

    /// Kills the daemon with `SIGKILL` and reaps it.
    pub fn kill(&mut self) -> Fallible {
        if let Some(mut daemon) = self.daemon.take() {
            daemon.kill()?;
            daemon.wait()?;
        }
        Ok(())
    }

    /// The daemon's log so far.
    pub fn log(&self) -> String {
        fs::read_to_string(self.state.join("daemon.log")).unwrap_or_default()
    }

    /// Starts a CLI command in the background.
    pub fn spawn(&self, args: &[&str]) -> Fallible<Child> {
        self.spawn_with(args, &[])
    }

    /// Starts a CLI command in the background with `vars` added to its environment.
    pub fn spawn_with(&self, args: &[&str], vars: &[(&str, &Path)]) -> Fallible<Child> {
        Ok(self
            .env
            .command(env!("CARGO_BIN_EXE_fabric"), &self.state)
            .args(args)
            .envs(vars.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?)
    }

    /// Runs the job here under a fake agent that stays alive after its
    /// turn, as a long session does, and returns the CLI that waits on it
    /// and the agent's pid once the node records it as running.
    pub fn run_lingering(&self, job: &str) -> Fallible<(Child, i32)> {
        let pid_file = self.state.join("harness.pid");
        let _ = fs::remove_file(&pid_file);
        let linger = Path::new("1");
        let running = self.spawn_with(
            &["job", "run", job, "--prompt", "two"],
            &[("FAKE_LINGER", linger), ("FAKE_PID_FILE", &pid_file)],
        )?;
        let started = Instant::now();
        loop {
            if self
                .ok(&["job", "status", job])?
                .contains("harness: running as pid")
            {
                break;
            }
            if started.elapsed() > FAULT_TIMEOUT {
                return Err("the lingering harness never registered".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid = fs::read_to_string(&pid_file)?.trim().parse()?;
        Ok((running, pid))
    }

    /// Runs a CLI command to its end.
    pub fn run(&self, args: &[&str]) -> Fallible<Output> {
        finish(self.spawn(args)?, COMMAND_TIMEOUT)
    }

    /// Runs a CLI command that must succeed and returns what it printed.
    pub fn ok(&self, args: &[&str]) -> Fallible<String> {
        let output = self.run(args)?;
        let text = text(&output);
        if output.status.success() {
            Ok(text)
        } else {
            Err(format!(
                "{} `fabric {}` failed:\n{text}\n--- daemon log ---\n{}",
                self.name,
                args.join(" "),
                self.log()
            )
            .into())
        }
    }

    /// Runs a CLI command that must fail and returns what it printed.
    pub fn fails(&self, args: &[&str]) -> Fallible<String> {
        let output = self.run(args)?;
        let text = text(&output);
        if output.status.success() {
            Err(format!(
                "{} `fabric {}` succeeded:\n{text}",
                self.name,
                args.join(" ")
            )
            .into())
        } else {
            Ok(text)
        }
    }

    /// What `fabric job status` says about the lease on this node.
    pub fn lease(&self, job: &str) -> Fallible<Lease> {
        let status = self.ok(&["job", "status", job])?;
        let owner_line = line_after(&status, "owner: ")?;
        let (owner, epoch) = owner_line
            .split_once(" at epoch ")
            .ok_or("the owner line names no epoch")?;
        let view = line_after(&status, "this node: ")?;
        Ok(Lease {
            owner: owner.trim_end_matches(" (this node)").to_owned(),
            epoch: epoch.parse()?,
            view: view
                .split([';', ' '])
                .next()
                .ok_or("the view line is empty")?
                .to_owned(),
        })
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

/// The lease as one node sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// The owner of record.
    pub owner: String,
    /// The current epoch.
    pub epoch: u64,
    /// How this node relates to it: owner, departing, departed or replica.
    pub view: String,
}

fn line_after(text: &str, prefix: &str) -> Fallible<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(str::to_owned)
        .ok_or_else(|| format!("no line starting {prefix:?} in:\n{text}").into())
}

/// Everything a command printed, standard output first.
pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Waits for a background command, and kills it past `timeout`.
///
/// Its output is read while it runs: a pipe can hold as little as 512
/// bytes on a busy Mac, and a command that cannot write never exits.
pub fn finish(child: Child, timeout: Duration) -> Fallible<Output> {
    let pid =
        rustix::process::Pid::from_raw(i32::try_from(child.id())?).ok_or("a child has no pid")?;
    let (done, waited) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait_with_output());
    });
    if let Ok(output) = waited.recv_timeout(timeout) {
        return Ok(output?);
    }
    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    let output = waited.recv()??;
    Err(format!("a command ran past {timeout:?}:\n{}", text(&output)).into())
}

/// True while a process with `pid` exists.
pub fn alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

/// Waits until `path` exists.
pub fn wait_for(path: &Path, what: &str) -> Fallible {
    let started = Instant::now();
    while !path.exists() {
        if started.elapsed() > FAULT_TIMEOUT {
            return Err(format!("{what} never happened").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// Pairs two started nodes the way the gate Macs pair: `a`, behind a NAT,
/// knows where `b` listens; `b` trusts `a` and has no address to dial it, so
/// every connection in the test is one `a` opened.
pub fn pair(a: &Node, b: &Node) -> Fallible {
    a.ok(&["node", "add", &b.id, &b.addr()])?;
    b.ok(&["node", "add", &a.id])?;
    Ok(())
}

/// The work a job holds after one turn: a staged file, an unstaged edit,
/// an untracked file and an ignored precious file, each of which a move
/// must carry exactly.
#[derive(Debug, PartialEq, Eq)]
pub struct Work {
    /// `git status --porcelain=v1` in the root.
    pub status: String,
    /// The edited tracked file.
    pub readme: String,
    /// The ignored file that is nobody's cache.
    pub precious: String,
}

/// Leaves work of every kind in the job's root.
pub fn edit(env: &Env, job: &str) -> Fallible<Work> {
    let work = env.work(job);
    fs::write(work.join("staged.txt"), "staged\n")?;
    git(&work, &["add", "staged.txt"])?;
    fs::write(work.join("README.md"), "hello\nunstaged edit\n")?;
    fs::write(work.join("notes.txt"), "untracked\n")?;
    fs::create_dir_all(work.join("scratch"))?;
    fs::write(work.join("scratch/plan.md"), "ignored but precious\n")?;
    read_work(env, job)
}

/// Reads back what `edit` left, wherever the job now lives.
pub fn read_work(env: &Env, job: &str) -> Fallible<Work> {
    let work = env.work(job);
    let output = Command::new("git")
        .current_dir(&work)
        .args(["status", "--porcelain=v1"])
        .output()?;
    if !output.status.success() {
        return Err(format!("git status failed in {}", work.display()).into());
    }
    Ok(Work {
        status: String::from_utf8(output.stdout)?,
        readme: fs::read_to_string(work.join("README.md"))?,
        precious: fs::read_to_string(work.join("scratch/plan.md"))?,
    })
}
