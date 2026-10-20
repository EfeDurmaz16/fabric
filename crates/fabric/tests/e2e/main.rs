//! End to end: two `fabric serve` daemons on one machine over loopback, one
//! job made under the fake ACP agent, suspended on one node and resumed on
//! the other, on QUIC and on TCP with TLS. Then the move is interrupted at
//! each named fault point, by `SIGKILL` of either daemon or by a dropped or
//! duplicated answer, and after recovery exactly one node owns the job, the
//! two ledgers agree on the epoch, the departed copy refuses to run, and the
//! owner resumes the job with its workspace and its harness session whole.

mod world;

use std::fs;
use std::time::Duration;

use world::{Env, Fallible, Node, Wire, Work, edit, finish, pair, read_work, text, wait_for};

const AGENT: &str = "acp:fabric-fake-agent";
const WIRES: [Wire; 2] = [Wire::Quic, Wire::TcpTls];

/// How long a move the test interrupted may take to give up.
const GIVE_UP: Duration = Duration::from_mins(3);

struct Pair {
    env: std::sync::Arc<Env>,
    a: Node,
    b: Node,
}

/// Starts two paired nodes, arming `fault_a` on the first and `fault_b` on the second.
fn setup(wire: Wire, fault_a: Option<&str>, fault_b: Option<&str>) -> Fallible<Pair> {
    let env = Env::new()?;
    let mut a = Node::new(&env, "a", wire)?;
    let mut b = Node::new(&env, "b", wire)?;
    a.start(fault_a)?;
    b.start(fault_b)?;
    pair(&a, &b)?;
    Ok(Pair { env, a, b })
}

/// Makes the job on `a`, runs its first turn and leaves work of every kind in its root.
fn job_with_work(p: &Pair) -> Fallible<(String, Work)> {
    let made = p.a.ok(&[
        "job",
        "new",
        "--repo",
        p.env
            .repo
            .to_str()
            .ok_or("the repository path is not UTF-8")?,
        "--goal",
        "make the greeting louder",
        "--harness",
        AGENT,
    ])?;
    let job = made
        .lines()
        .next()
        .ok_or("job new printed nothing")?
        .to_owned();
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
    let refusal = other.fails(&["job", "run", job])?;
    assert!(
        refusal.contains("does not run") || refusal.contains("is owned by"),
        "{} refused for another reason:\n{refusal}",
        other.name
    );
    Ok(())
}

/// The owner resumes the job: the harness state must travel (the shared
/// home's copy is removed first, as on a second machine), the workspace
/// matches what was left, and the session takes one more turn.
fn assert_continues(p: &Pair, owner: &Node, job: &str, work: &Work, turns: &[&str]) -> Fallible {
    fs::remove_file(p.env.session_file())?;
    owner.ok(&["job", "resume", job, "--prompt", "next"])?;
    let now = read_work(&p.env, job)?;
    assert_eq!(
        now.status, work.status,
        "staged, unstaged and untracked changes"
    );
    assert_eq!(now.readme, work.readme, "the unstaged edit");
    assert_eq!(now.precious, work.precious, "the ignored precious file");
    let mut expected: Vec<String> = turns.iter().map(|t| (*t).to_owned()).collect();
    expected.push("next".to_owned());
    assert_eq!(p.env.prompts()?, expected, "the harness session continued");
    Ok(())
}

fn move_and_resume(wire: Wire) -> Fallible {
    let p = setup(wire, None, None)?;
    let (job, work) = job_with_work(&p)?;
    let doctor = p.a.ok(&["doctor", "--peer", &p.b.id, "--job", &job])?;
    assert!(doctor.contains("ok      home"), "{doctor}");
    p.a.ok(&["job", "approve", &job, "--note", "the plan reads right"])?;
    let suspended = p.a.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains("copied the ledger and the snapshot"),
        "{suspended}"
    );

    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
    assert_continues(&p, &p.b, &job, &work, &["one"])?;

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
    assert_continues(&p, &p.b, &job, &work, &["one"])
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
        assert_one_owner(&p.b, &p.a, &job, 2, "departed")?;
        let log = p.a.ok(&["job", "log", &job])?;
        assert_eq!(log.matches("MoveImported").count(), 1, "{log}");
        assert_continues(&p, &p.b, &job, &work, &["one"])?;
    }
    Ok(())
}

#[test]
fn e2e_a_steal_needs_force_and_fences_the_sleeping_owner() -> Fallible {
    let mut p = setup(Wire::TcpTls, None, None)?;
    let (job, work) = job_with_work(&p)?;
    p.a.ok(&["job", "suspend", &job])?;
    p.a.kill()?;
    let refused = p.b.fails(&["job", "steal", &job])?;
    assert!(refused.contains("--force"), "{refused}");
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
    assert_continues(&p, &p.b, &job, &work, &["one"])
}
