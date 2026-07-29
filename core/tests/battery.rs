//! Runs the full selftest battery without the libtest harness. libtest's
//! output capture intercepts `print!`-macro output from uutils commands
//! (inherited by spawned threads), which falsifies fd-redirection results —
//! so this test must own its process: harness = false in Cargo.toml.

fn main() {
    // The battery intentionally exercises early-closing pipelines (`head`,
    // `grep -q`). A staticlib host installs this policy in session_new; this
    // standalone test binary must do the same or the OS kills the entire
    // matrix before Brush can turn EPIPE into a stage exit status.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let dir = std::env::temp_dir().join(format!("yourshell_battery_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let report = ashellcore::selftest::run_selftest(&dir);
    print!("{report}");
    if report.contains("FAIL") || report.contains("FATAL") {
        std::process::exit(1);
    }
}
