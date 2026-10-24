//! `h2workbench`: the Harness² Workbench server and its administration commands.
//!
//! Everything lives in the `h2w_daemon` library so integration tests can build the real
//! router and server; this binary only hands control to [`h2w_daemon::cli::main`].

fn main() -> std::process::ExitCode {
    h2w_daemon::cli::main()
}
