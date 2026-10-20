//! End to end: two `fabric serve` daemons on one machine over loopback, one
//! job made under the fake ACP agent, suspended on one node and resumed on
//! the other, on QUIC and on TCP with TLS. Then the move is interrupted at
//! each named fault point, by `SIGKILL` of either daemon or by a dropped or
//! duplicated answer, and after recovery exactly one node owns the job, the
//! two ledgers agree on the epoch, the departed copy refuses to run, and the
//! owner resumes the job with its workspace and its harness session whole.
//! One more test runs a Claude job on one node beside a running `claude`.

mod world;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use fabric_env::materialize::TreeLock;

use world::{
    Env, Fallible, Node, OtherClaude, Wire, Work, alive, edit, finish, pair, read_work, text,
    wait_for,
};

const AGENT: &str = "acp:fabric-fake-agent";
const WIRES: [Wire; 2] = [Wire::Quic, Wire::TcpTls];

/// How long a move the test interrupted may take to give up.
const GIVE_UP: Duration = Duration::from_mins(3);

struct Pair {
    env: std::sync::Arc<Env>,
    a: Node,
    b: Node,
}

/// Starts two paired nodes, arming `fault_a` on the first and `fault_b` on
/// the second. They report the Node versions the gate Macs had, 22 on the
/// first and 26 on the second, so every move crosses Node majors.
fn setup(wire: Wire, fault_a: Option<&str>, fault_b: Option<&str>) -> Fallible<Pair> {
    let env = Env::new()?;
    let mut a = Node::new(&env, "a", wire)?;
    let mut b = Node::new(&env, "b", wire)?;
    a.fake_node("v22.23.2", "127")?;
    b.fake_node("v26.0.0", "147")?;
    a.start(fault_a)?;
    b.start(fault_b)?;
    pair(&a, &b)?;
    Ok(Pair { env, a, b })
}

/// Makes a job from the fixture repository on `node` under `harness` and returns its id.
fn new_job(env: &Env, node: &Node, harness: &str) -> Fallible<String> {
    let made = node.ok(&[
        "job",
        "new",
        "--repo",
        env.repo
            .to_str()
            .ok_or("the repository path is not UTF-8")?,
        "--goal",
        "make the greeting louder",
        "--harness",
        harness,
    ])?;
    Ok(made
        .lines()
        .next()
        .ok_or("job new printed nothing")?
        .to_owned())
}

/// Makes the job on `a`, runs its first turn and leaves work of every kind in its root.
fn job_with_work(p: &Pair) -> Fallible<(String, Work)> {
    let job = new_job(&p.env, &p.a, AGENT)?;
    p.a.ok(&["job", "run", &job, "--prompt", "one"])?;
    let work = edit(&p.env, &job)?;
    Ok((job, work))
}

/// Both nodes name `owner` at `epoch`, the owner says so, and the other
/// node, `other_view` (departed or replica), refuses to run the job.
fn assert_one_owner(
    owner: &Node,
    other: &Node,
    job: &str,
    epoch: u64,
    other_view: &str,
) -> Fallible {
    let here = owner.lease(job)?;
    let there = other.lease(job)?;
    assert_eq!(here.owner, owner.id, "{} names another owner", owner.name);
    assert_eq!(there.owner, owner.id, "{} names another owner", other.name);
    assert_eq!(
        (here.epoch, there.epoch),
        (epoch, epoch),
        "the epochs differ"
    );
    assert_eq!(here.view, "owner");
    assert_eq!(there.view, other_view);
    let refused = other.run(&["job", "run", job])?;
    let refusal = text(&refused);
    assert!(
        refusal.contains("does not run") || refusal.contains("is owned by"),
        "{} refused for another reason:\n{refusal}",
        other.name
    );
    assert_eq!(refused.status.code(), Some(81), "not the owner:\n{refusal}");
    Ok(())
}

/// The owner resumes the job: the harness state must travel (the shared
/// home's copy is removed first, as on a second machine), the workspace
/// matches what was left, and the session takes one more turn. Returns what
/// the resume printed.
fn assert_continues(
    p: &Pair,
    owner: &Node,
    job: &str,
    work: &Work,
    turns: &[&str],
) -> Fallible<String> {
    fs::remove_file(p.env.session_file())?;
    let resumed = owner.ok(&["job", "resume", job, "--prompt", "next"])?;
    let now = read_work(&p.env, job)?;
    assert_eq!(
        now.status, work.status,
        "staged, unstaged and untracked changes"
    );
    assert_eq!(now.readme, work.readme, "the unstaged edit");
    assert_eq!(now.precious, work.precious, "the ignored precious file");
    assert_eq!(
        now.deps, work.deps,
        "the node_modules no lockfile can rebuild"
    );
    assert_eq!(now.build, work.build, "the build output");
    let mut expected: Vec<String> = turns.iter().map(|t| (*t).to_owned()).collect();
    expected.push("next".to_owned());
    assert_eq!(p.env.prompts()?, expected, "the harness session continued");
    Ok(resumed)
}

/// The count of `class` in a restore summary line, such as 5 for `tracked: 5 files`.
fn restored_files(summary: &str, class: &str) -> Fallible<u64> {
    let after = summary
        .split_once(&format!("{class}: "))
        .ok_or_else(|| format!("no {class} count in: {summary}"))?
        .1;
    let count = after
        .split_once(' ')
        .ok_or_else(|| format!("no number after {class}: {summary}"))?
        .0;
    Ok(count.parse()?)
}

/// What the move must decide for the fixture's ignored paths between the
/// two nodes, with Node 22 on one and 26 on the other.
const DECISIONS: [&str; 3] = [
    "leave   .env  (it matches a secret pattern",
    "carry   dist  (same os and arch",
    "carry   node_modules  (no lockfile or setup step makes it again",
];

fn move_and_resume(wire: Wire) -> Fallible {
    let p = setup(wire, None, None)?;
    let (job, work) = job_with_work(&p)?;
    let doctor = p.a.ok(&["doctor", "--peer", &p.b.id, "--job", &job])?;
    assert!(doctor.contains("ok      home"), "{doctor}");
    assert!(doctor.contains("no snapshot yet"), "{doctor}");
    p.a.ok(&["job", "approve", &job, "--note", "the plan reads right"])?;
    let suspended = p.a.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains("copied the ledger and the snapshot"),
        "{suspended}"
    );
    assert!(
        suspended.contains("left behind (secret pattern): .env"),
        "{suspended}"
    );
    assert!(
        p.a.ok(&["job", "status", &job])?
            .contains("session: captured in snapshot"),
        "the fake agent's session is in the snapshot"
    );

    // Gate findings F6 and F7: the Node majors differ, and the hand-made
    // node_modules and the dist/ still travel; doctor and move say so.
    let doctor = p.a.ok(&["doctor", "--peer", &p.b.id, "--job", &job])?;
    assert!(doctor.contains("rebuild node.version"), "{doctor}");
    let moved = p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    for decision in DECISIONS {
        assert!(doctor.contains(decision), "doctor: {decision}:\n{doctor}");
        assert!(moved.contains(decision), "move: {decision}:\n{moved}");
    }
    assert!(!moved.contains("no harness session captured"), "{moved}");
    assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
    let resumed = assert_continues(&p, &p.b, &job, &work, &["one"])?;
    for decision in DECISIONS {
        assert!(resumed.contains(decision), "resume: {decision}:\n{resumed}");
    }
    assert!(
        !p.env.work(&job).join(".env").exists(),
        "the secret travelled"
    );
    // Gate finding F8: the summary counts every file restore wrote: the
    // four committed files and the staged one, the untracked notes, and the
    // precious file, the node_modules file and the dist file.
    let summary = resumed
        .lines()
        .find(|line| line.starts_with("restored snapshot"))
        .ok_or_else(|| format!("no restore summary in:\n{resumed}"))?;
    assert_eq!(restored_files(summary, "tracked")?, 5, "{summary}");
    assert_eq!(restored_files(summary, "untracked")?, 1, "{summary}");
    assert_eq!(restored_files(summary, "ignored")?, 3, "{summary}");
    assert!(restored_files(summary, "harness state")? > 0, "{summary}");

    // The way back is a pull typed on the other node, which suspends the
    // job there and so captures the session from that node's home.
    p.a.ok(&["job", "resume", &job, "--prompt", "back"])?;
    assert_one_owner(&p.a, &p.b, &job, 3, "departed")?;
    assert_eq!(p.env.prompts()?, ["one", "next", "back"]);
    assert_eq!(read_work(&p.env, &job)?.readme, work.readme);

    let log = p.b.ok(&["job", "log", &job])?;
    assert!(log.contains("DecisionRecorded: approved: the plan reads right"));
    assert_eq!(log.matches("MoveImported").count(), 2, "{log}");
    Ok(())
}

/// Gate finding F9: a job whose harness never ran has no session to
/// capture. Status and move say so before the job leaves, so nobody expects
/// the conversation to continue on the target.
#[test]
fn e2e_a_job_with_no_session_says_so_before_it_moves() -> Fallible {
    let p = setup(Wire::Quic, None, None)?;
    let job = new_job(&p.env, &p.a, AGENT)?;
    p.a.ok(&["job", "suspend", &job])?;
    let line = "no harness session captured; the conversation will not continue on the target";
    let status = p.a.ok(&["job", "status", &job])?;
    assert!(status.contains(&format!("session: {line}")), "{status}");
    let moved = p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    assert!(moved.contains(line), "{moved}");
    Ok(())
}

#[test]
fn e2e_a_job_moves_and_resumes_over_quic() -> Fallible {
    move_and_resume(Wire::Quic)
}

#[test]
fn e2e_a_job_moves_and_resumes_over_tcp_tls() -> Fallible {
    move_and_resume(Wire::TcpTls)
}

/// Which node a step acts on.
#[derive(Clone, Copy)]
enum Side {
    A,
    B,
}

impl Pair {
    fn node(&mut self, side: Side) -> &mut Node {
        match side {
            Side::A => &mut self.a,
            Side::B => &mut self.b,
        }
    }
}

/// Starts a move on `a`, waits for `armed` to reach its fault point, and kills `victim`.
fn kill_at_fault(p: &mut Pair, job: &str, armed: Side, victim: Side) -> Fallible<String> {
    let moving = p.a.spawn(&["job", "move", job, "--to", &p.b.id])?;
    wait_for(&p.node(armed).signal(), "the fault point")?;
    p.node(victim).kill()?;
    let output = finish(moving, GIVE_UP)?;
    assert!(
        !output.status.success(),
        "the interrupted move reported success"
    );
    Ok(text(&output))
}

/// The source dies before anything about ownership is written: the job
/// never left it, and the move asked again completes.
fn source_killed_before_departing(wire: Wire, point: &str) -> Fallible {
    let mut p = setup(wire, Some(point), None)?;
    let (job, work) = job_with_work(&p)?;
    kill_at_fault(&mut p, &job, Side::A, Side::A)?;
    p.a.start(None)?;
    let lease = p.a.lease(&job)?;
    assert_eq!((lease.owner.as_str(), lease.epoch), (p.a.id.as_str(), 1));
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
    assert_continues(&p, &p.b, &job, &work, &["one"])?;
    Ok(())
}

#[test]
fn e2e_source_killed_after_quiesce() -> Fallible {
    for wire in WIRES {
        source_killed_before_departing(wire, "after_quiesce")?;
    }
    Ok(())
}

#[test]
fn e2e_source_killed_after_snapshot_publish() -> Fallible {
    for wire in WIRES {
        source_killed_before_departing(wire, "after_snapshot_publish")?;
    }
    Ok(())
}

#[test]
fn e2e_source_killed_after_departing_write() -> Fallible {
    for wire in WIRES {
        let mut p = setup(wire, Some("after_departing_write"), None)?;
        let (job, work) = job_with_work(&p)?;
        kill_at_fault(&mut p, &job, Side::A, Side::A)?;
        p.a.start(None)?;
        // MoveStarted is durable and the target never saw an offer: the
        // source is frozen, not runnable, until the target decides.
        assert_eq!(p.a.lease(&job)?.view, "departing");
        let refusal = p.a.fails(&["job", "run", &job])?;
        assert!(refusal.contains("departing"), "{refusal}");
        p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

#[test]
fn e2e_target_killed_mid_transfer() -> Fallible {
    for wire in WIRES {
        let mut p = setup(wire, None, Some("mid_transfer"))?;
        let (job, work) = job_with_work(&p)?;
        kill_at_fault(&mut p, &job, Side::B, Side::B)?;
        p.b.start(None)?;
        assert_eq!(p.a.lease(&job)?.view, "owner", "a transfer moves no lease");
        p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

#[test]
fn e2e_source_killed_mid_transfer() -> Fallible {
    for wire in WIRES {
        let mut p = setup(wire, None, Some("mid_transfer:stall"))?;
        let (job, work) = job_with_work(&p)?;
        kill_at_fault(&mut p, &job, Side::B, Side::A)?;
        p.a.start(None)?;
        assert_eq!(p.a.lease(&job)?.view, "owner", "a transfer moves no lease");
        p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

/// A run typed on the source while the target copies the job: the copy is
/// then out of date, so the move must not depart from it. It refuses, the
/// source keeps the job, and the move typed again carries the new turn.
#[test]
fn e2e_a_run_during_the_transfer_stops_the_move() -> Fallible {
    let p = setup(Wire::TcpTls, None, Some("mid_transfer:hold"))?;
    let (job, work) = job_with_work(&p)?;
    let moving = p.a.spawn(&["job", "move", &job, "--to", &p.b.id])?;
    wait_for(&p.b.signal(), "the transfer")?;
    p.a.ok(&["job", "run", &job, "--prompt", "two"])?;
    fs::remove_file(p.b.signal())?;
    let output = finish(moving, GIVE_UP)?;
    let printed = text(&output);
    assert!(!output.status.success(), "the move departed:\n{printed}");
    assert!(printed.contains("ran on this node while"), "{printed}");
    assert_eq!(p.a.lease(&job)?.view, "owner");
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
    assert_continues(&p, &p.b, &job, &work, &["one", "two"])?;
    Ok(())
}

#[test]
fn e2e_target_killed_after_import_receipt() -> Fallible {
    for wire in WIRES {
        let mut p = setup(wire, None, Some("after_import_receipt"))?;
        let (job, work) = job_with_work(&p)?;
        let failed = kill_at_fault(&mut p, &job, Side::B, Side::B)?;
        assert!(failed.contains("departing"), "{failed}");
        // The T3 Code case: the import is durable and its answer is lost.
        // The source must not run, and no retry makes it runnable.
        let refusal = p.a.fails(&["job", "run", &job])?;
        assert!(refusal.contains("departing"), "{refusal}");
        p.b.start(None)?;
        // The move typed again learns the decision and reports it done.
        let again = p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
        assert!(again.contains("already owns the job at epoch 2"), "{again}");
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

#[test]
fn e2e_a_dropped_import_answer_converges_on_the_same_record() -> Fallible {
    for wire in WIRES {
        let p = setup(wire, None, Some("after_import_receipt:drop"))?;
        let (job, work) = job_with_work(&p)?;
        p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
        assert!(
            p.b.log().contains("dropping the answer"),
            "the fault did not fire"
        );
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        let log = p.b.ok(&["job", "log", &job])?;
        assert_eq!(log.matches("MoveImported").count(), 1, "{log}");
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

#[test]
fn e2e_a_duplicated_import_answer_changes_nothing() -> Fallible {
    for wire in WIRES {
        let p = setup(wire, Some("after_import_receipt:dup"), None)?;
        let (job, work) = job_with_work(&p)?;
        p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
        assert!(
            p.a.log().contains("fault point reached"),
            "the fault did not fire"
        );
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        let log = p.a.ok(&["job", "log", &job])?;
        assert_eq!(log.matches("MoveImported").count(), 1, "{log}");
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

/// A user edits the tree by hand after a suspend, then moves the job. The
/// move must capture the edit, so it arrives on the target and nothing is
/// left only in the departed copy.
#[test]
fn e2e_a_hand_edit_after_a_suspend_moves_with_the_job() -> Fallible {
    let p = setup(Wire::TcpTls, None, None)?;
    let (job, mut work) = job_with_work(&p)?;
    p.a.ok(&["job", "suspend", &job])?;
    let edited = "hello\nedited by hand after the suspend\n";
    fs::write(p.env.work(&job).join("README.md"), edited)?;
    let moved = p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    assert!(moved.contains("capturing it again"), "{moved}");
    assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
    edited.clone_into(&mut work.readme);
    assert_continues(&p, &p.b, &job, &work, &["one"])?;
    Ok(())
}

#[test]
fn e2e_a_steal_needs_force_and_fences_the_sleeping_owner() -> Fallible {
    let mut p = setup(Wire::TcpTls, None, None)?;
    let (job, work) = job_with_work(&p)?;
    p.a.ok(&["job", "suspend", &job])?;
    p.a.kill()?;
    let refused = p.b.run(&["job", "steal", &job])?;
    assert_eq!(refused.status.code(), Some(84), "{}", text(&refused));
    assert!(text(&refused).contains("--force"), "{}", text(&refused));
    let stolen = p.b.ok(&[
        "job",
        "steal",
        &job,
        "--force",
        "--reason",
        "the pro sleeps",
    ])?;
    assert!(stolen.contains("no work is lost"), "{stolen}");
    // On waking, the stolen-from node learns the steal from its peer.
    p.a.start(None)?;
    assert_one_owner(&p.b, &p.a, &job, 2, "replica")?;
    assert_continues(&p, &p.b, &job, &work, &["one"])?;
    Ok(())
}

/// A move stops the harness that runs on its source, whichever way it is
/// typed: a push from the node that runs it, and a pull from the other
/// node. Each stopped harness exits by its signal, and its last turn is in
/// the snapshot the job moves with.
#[test]
fn e2e_a_move_stops_the_harness_running_on_its_source() -> Fallible {
    let p = setup(Wire::TcpTls, None, None)?;
    let (job, work) = job_with_work(&p)?;
    let (running, pid) = p.a.run_lingering(&job)?;
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    let output = finish(running, GIVE_UP)?;
    assert_eq!(output.status.code(), Some(128 + 15), "{}", text(&output));
    assert!(!alive(pid), "the pushed job's harness still runs");
    assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
    assert_continues(&p, &p.b, &job, &work, &["one", "two"])?;

    let (running, pid) = p.b.run_lingering(&job)?;
    p.a.ok(&["job", "resume", &job, "--prompt", "back"])?;
    let output = finish(running, GIVE_UP)?;
    assert_eq!(output.status.code(), Some(128 + 15), "{}", text(&output));
    assert!(
        !alive(pid),
        "the pulled job's harness still runs on its holder"
    );
    assert_one_owner(&p.a, &p.b, &job, 3, "departed")?;
    assert_eq!(p.env.prompts()?, ["one", "two", "next", "two", "back"]);
    Ok(())
}

/// Moves the job to `b` so that a pull typed on `a` brings it back.
fn job_on_b(p: &Pair) -> Fallible<(String, Work)> {
    let (job, work) = job_with_work(p)?;
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    Ok((job, work))
}

/// A pull broken on the holder after it wrote `MoveStarted` toward the
/// puller: the holder is frozen, not runnable, and the pull typed again
/// converges on one owner.
#[test]
fn e2e_holder_killed_after_departing_write_in_a_pull() -> Fallible {
    let mut p = setup(Wire::TcpTls, None, Some("after_departing_write"))?;
    let (job, work) = job_on_b(&p)?;
    let pulling = p.a.spawn(&["job", "resume", &job, "--prompt", "back"])?;
    wait_for(&p.b.signal(), "the fault point")?;
    p.b.kill()?;
    let output = finish(pulling, GIVE_UP)?;
    assert!(!output.status.success(), "{}", text(&output));
    p.b.start(None)?;
    assert_eq!(p.b.lease(&job)?.view, "departing");
    let refusal = p.b.fails(&["job", "run", &job])?;
    assert!(refusal.contains("departing"), "{refusal}");
    fs::remove_file(p.env.session_file())?;
    p.a.ok(&["job", "resume", &job, "--prompt", "back"])?;
    assert_one_owner(&p.a, &p.b, &job, 3, "departed")?;
    assert_eq!(read_work(&p.env, &job)?, work);
    assert_eq!(p.env.prompts()?, ["one", "back"]);
    Ok(())
}

/// A pull broken on the puller after it learned the holder's `MoveStarted`
/// and before it imported: nothing owns the job but the holder's open
/// move, and the pull typed again imports it.
#[test]
fn e2e_puller_killed_before_import() -> Fallible {
    let mut p = setup(Wire::TcpTls, Some("before_import"), None)?;
    let (job, work) = job_on_b(&p)?;
    let pulling = p.a.spawn(&["job", "resume", &job, "--prompt", "back"])?;
    wait_for(&p.a.signal(), "the fault point")?;
    p.a.kill()?;
    let output = finish(pulling, GIVE_UP)?;
    assert!(!output.status.success(), "{}", text(&output));
    p.a.start(None)?;
    assert_eq!(p.b.lease(&job)?.view, "departing");
    assert_ne!(p.a.lease(&job)?.view, "owner");
    fs::remove_file(p.env.session_file())?;
    p.a.ok(&["job", "resume", &job, "--prompt", "back"])?;
    assert_one_owner(&p.a, &p.b, &job, 3, "departed")?;
    assert_eq!(read_work(&p.env, &job)?, work);
    assert_eq!(p.env.prompts()?, ["one", "back"]);
    Ok(())
}

/// The owner suspended, then resumed and runs the job while another node
/// steals it, so the two ledgers fork. The thief pushes its steal over the
/// session the owner keeps open; the owner finds that the chains do not
/// join at a higher epoch, freezes the job, stops its harness and refuses
/// to run it again. Its workspace stays at the root as an orphan, which on
/// this shared disk is also the thief's root, so the thief's ownership is
/// asserted from its ledger.
#[test]
fn e2e_a_steal_after_a_resume_freezes_the_owner_and_stops_its_harness() -> Fallible {
    let p = setup(Wire::TcpTls, None, None)?;
    let (job, _) = job_with_work(&p)?;
    p.a.ok(&["job", "suspend", &job])?;
    let (running, pid) = p.a.run_lingering(&job)?;
    p.b.ok(&["job", "steal", &job, "--force"])?;
    let output = finish(running, GIVE_UP)?;
    assert_eq!(output.status.code(), Some(128 + 15), "{}", text(&output));
    assert!(
        text(&output).contains("stopped by signal 15"),
        "{}",
        text(&output)
    );
    assert!(!alive(pid), "the owner's harness outlived the steal");
    assert_eq!(p.a.lease(&job)?.view, "frozen");
    let refusal = p.a.run(&["job", "run", &job])?;
    assert_eq!(refusal.status.code(), Some(82), "{}", text(&refusal));
    assert!(
        text(&refusal).contains("is frozen on this node"),
        "{}",
        text(&refusal)
    );
    let thief = p.b.lease(&job)?;
    assert_eq!(
        (thief.owner.as_str(), thief.epoch, thief.view.as_str()),
        (p.b.id.as_str(), 2, "owner")
    );
    Ok(())
}

/// As above, with the owner's daemon up but out of reach when the steal
/// happens, as a laptop on another network: nothing tells it, so its
/// harness keeps running until the next `run` catches up with the thief,
/// finds the fork, stops the harness and refuses.
#[test]
fn e2e_an_owner_that_stayed_up_through_a_steal_refuses_to_run() -> Fallible {
    let mut p = setup(Wire::TcpTls, None, None)?;
    let (job, _) = job_with_work(&p)?;
    p.a.ok(&["job", "suspend", &job])?;
    let (running, pid) = p.a.run_lingering(&job)?;
    p.b.kill()?;
    p.b.start(None)?;
    let stolen = p.b.ok(&["job", "steal", &job, "--force"])?;
    assert!(stolen.contains("has not seen the steal"), "{stolen}");
    assert!(alive(pid), "nothing could have told the owner yet");
    let refusal = p.a.fails(&["job", "run", &job])?;
    assert!(refusal.contains("is frozen on this node"), "{refusal}");
    let output = finish(running, GIVE_UP)?;
    assert!(!output.status.success(), "{}", text(&output));
    assert!(!alive(pid), "the owner's harness outlived the fork");
    Ok(())
}

/// The exit codes of ADR 0016 that the other tests do not reach: scripts
/// and the triggers of ADR 0008 read them, so each is pinned here.
#[test]
fn e2e_refusals_exit_with_their_documented_codes() -> Fallible {
    let p = setup(Wire::TcpTls, None, None)?;
    let (job, _) = job_with_work(&p)?;

    let nowhere = Node::new(&p.env, "nowhere", Wire::TcpTls)?;
    fs::set_permissions(nowhere.state(), fs::Permissions::from_mode(0o700))?;
    let no_daemon = nowhere.run(&["job", "ls"])?;
    assert_eq!(no_daemon.status.code(), Some(80), "{}", text(&no_daemon));

    // Another run of the workspace holds it while it starts its harness.
    let held = TreeLock::acquire(&p.a.state().join("locks"), &p.env.work(&job))?;
    let busy = p.a.run(&["job", "run", &job])?;
    assert_eq!(busy.status.code(), Some(85), "{}", text(&busy));
    drop(held);

    let (running, pid) = p.a.run_lingering(&job)?;
    let twice = p.a.run(&["job", "run", &job])?;
    assert_eq!(twice.status.code(), Some(85), "{}", text(&twice));
    p.a.ok(&["job", "suspend", &job])?;
    finish(running, GIVE_UP)?;
    assert!(!alive(pid), "the suspend left the harness running");
    Ok(())
}

/// Gate finding F1: a user keeps Claude Code open, so a `claude` process
/// runs when a Claude job starts. The trust pre-seed must then leave
/// `~/.claude.json` to that Claude, say so in one line, and launch; it used
/// to refuse the run. The process the pre-seed's `pgrep` probe finds is a
/// real one named `claude`, so the test holds on a machine with no Claude.
#[test]
fn e2e_a_claude_job_launches_and_warns_while_another_claude_runs() -> Fallible {
    let env = Env::new()?;
    env.install_fake_claude()?;
    let config = env.home.join(".claude.json");
    let untrusted = "{\"projects\":{}}\n";
    fs::write(&config, untrusted)?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    a.start(None)?;
    let job = new_job(&env, &a, "claude")?;
    let other = OtherClaude::start(&env)?;
    let ran = a.ok(&["job", "run", &job, "--prompt", "one"])?;
    let warning = ran
        .lines()
        .find(|line| line.starts_with("warning: Fabric did not trust"))
        .ok_or_else(|| format!("no trust warning in:\n{ran}"))?;
    let pids = warning
        .split_once("claude runs as pid ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(pids, _)| pids)
        .ok_or_else(|| format!("the warning names no running claude: {warning}"))?;
    assert!(
        pids.split(", ").any(|pid| pid == other.pid().to_string()),
        "{warning}"
    );
    let launched = env.fake_claude_args()?;
    assert!(
        launched.iter().any(|arg| arg == "--session-id") && launched.iter().any(|arg| arg == "one"),
        "{launched:?}"
    );
    assert_eq!(
        fs::read_to_string(&config)?,
        untrusted,
        "the config was written while claude ran"
    );
    Ok(())
}
