//! Runs the full selftest battery without the libtest harness. libtest's
//! output capture intercepts `print!`-macro output from uutils commands
//! (inherited by spawned threads), which falsifies fd-redirection results —
//! so this test must own its process: harness = false in Cargo.toml.

fn main() {
    let dir = std::env::temp_dir().join(format!("yourshell_battery_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let report = ashellcore::selftest::run_selftest(&dir);
    print!("{report}");
    if report.contains("FAIL") || report.contains("FATAL") {
        std::process::exit(1);
    }
}
