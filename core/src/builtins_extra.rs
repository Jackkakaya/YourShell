//! Library-backed in-process commands. Policy: no hand-written command
//! implementations — commands come from uutils (see `uutils_adapter`) or
//! ecosystem crates; this module only hosts thin library adapters.

use std::io::{Read, Write};
use std::path::PathBuf;

use brush_core::{builtins, ExecutionResult};
use clap::Parser;

/// Search input for lines matching a regular expression.
#[derive(Parser)]
pub struct GrepCommand {
    /// Case-insensitive matching.
    #[arg(short = 'i')]
    ignore_case: bool,
    /// Invert the match.
    #[arg(short = 'v')]
    invert: bool,
    /// Print match counts instead of lines.
    #[arg(short = 'c')]
    count: bool,
    /// Print only the matched part of each line.
    #[arg(short = 'o')]
    only_matching: bool,
    /// Prefix output lines with line numbers.
    #[arg(short = 'n')]
    line_numbers: bool,
    pattern: String,
    files: Vec<String>,
}

impl builtins::Command for GrepCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let pattern = if self.ignore_case {
            format!("(?i){}", self.pattern)
        } else {
            self.pattern.clone()
        };
        let re = match regex_lite::Regex::new(&pattern) {
            Ok(re) => re,
            Err(e) => {
                writeln!(context.stderr(), "grep: bad pattern: {e}")?;
                return Ok(ExecutionResult::new(2));
            }
        };

        let mut inputs: Vec<String> = Vec::new();
        if self.files.is_empty() {
            let mut buf = String::new();
            context.stdin().read_to_string(&mut buf)?;
            inputs.push(buf);
        } else {
            for f in &self.files {
                inputs.push(std::fs::read_to_string(absolutize(&cwd, f)).unwrap_or_default());
            }
        }

        let mut out = context.stdout();
        let mut matched_any = false;
        for input in &inputs {
            let mut count = 0usize;
            for (idx, line) in input.lines().enumerate() {
                let is_match = re.is_match(line);
                if is_match != self.invert {
                    matched_any = true;
                    count += 1;
                    if self.count {
                        continue;
                    }
                    if self.only_matching && !self.invert {
                        for m in re.find_iter(line) {
                            writeln!(out, "{}", m.as_str())?;
                        }
                    } else if self.line_numbers {
                        writeln!(out, "{}:{line}", idx + 1)?;
                    } else {
                        writeln!(out, "{line}")?;
                    }
                }
            }
            if self.count {
                writeln!(out, "{count}")?;
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(if matched_any { 0 } else { 1 }))
    }
}

fn absolutize(cwd: &std::path::Path, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
