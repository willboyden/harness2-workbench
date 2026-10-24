//! `h2studio`: the Harness² Studio server and its administration commands.
//!
//! Everything lives in the `h2s_daemon` library so integration tests can build the real
//! router and server; this binary only hands control to [`h2s_daemon::cli::main`].

fn main() -> std::process::ExitCode {
    h2s_daemon::cli::main()
}
