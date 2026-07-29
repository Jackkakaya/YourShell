//! Flag-coverage audit. Runs outside the libtest harness for the same reason as
//! the battery: libtest's output capture swallows `print!`-macro output from
//! in-process commands, which is exactly what this probe reads.
//!
//! This is a visibility gate, not a correctness test. It exists because the one
//! time a command shipped with a partial CLI (`grep` with 9 of ~50 flags), it
//! was found only when a missing `-E` killed a whole selftest run.

fn main() {
    let dir = std::env::temp_dir().join(format!("yourshell_flagaudit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let report = ashellcore::selftest::run_flag_coverage(&dir);
    print!("{report}");
    // Fail on a command that is mostly missing — the "half-finished command"
    // signature. Individual known gaps (upstream diffutils lacks -i/-w/-B) stay
    // visible in the report without breaking the build.
    let bad: Vec<&str> = report
        .lines()
        .filter(|l| {
            l.split_once('(')
                .and_then(|(_, rest)| rest.split('%').next())
                .and_then(|p| p.parse::<u32>().ok())
                .is_some_and(|pct| pct < 60)
        })
        .collect();
    if !bad.is_empty() {
        eprintln!(
            "\nFAIL: command(s) below 60% flag coverage:\n{}",
            bad.join("\n")
        );
        std::process::exit(1);
    }
}
