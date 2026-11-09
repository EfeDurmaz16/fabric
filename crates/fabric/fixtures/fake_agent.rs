//! The fake ACP agent of fabric-harness's tests, as a program the
//! end-to-end test installs on `PATH` and launches as a harness.

#[path = "../../fabric-harness/tests/fake_acp_agent/mod.rs"]
mod fake_acp_agent;

fn main() -> std::process::ExitCode {
    // The test's run at install (`write_executable`, `link_executable` in
    // tests/e2e/world.rs) only pays macOS's first-run check.
    if std::env::var_os("FABRIC_TOOL_WARM").is_some() {
        return std::process::ExitCode::SUCCESS;
    }
    match fake_acp_agent::serve() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
