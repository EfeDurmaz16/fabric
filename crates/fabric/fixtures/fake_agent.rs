//! The fake ACP agent of fabric-harness's tests, as a program the
//! end-to-end test installs on `PATH` and launches as a harness.

#[path = "../../fabric-harness/tests/fake_acp_agent/mod.rs"]
mod fake_acp_agent;

fn main() -> std::process::ExitCode {
    match fake_acp_agent::serve() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
