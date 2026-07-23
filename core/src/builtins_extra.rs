//! In-process implementations of commands that a traditional shell would
//! spawn as external processes. On iOS there is no fork/exec, so these are
//! registered as brush builtins and run inside the app process, reading and
//! writing through the shell's virtual fd table (real pipes, per-session).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

use brush_core::{builtins, ExecutionResult};
use clap::Parser;

/// List directory contents.
#[derive(Parser)]
pub struct LsCommand {
    /// Use long listing format.
    #[arg(short = 'l')]
    long: bool,
    /// Show hidden entries.
    #[arg(short = 'a')]
    all: bool,
    /// Paths to list.
    paths: Vec<String>,
}

impl builtins::Command for LsCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let targets: Vec<PathBuf> = if self.paths.is_empty() {
            vec![cwd.clone()]
        } else {
            self.paths.iter().map(|p| absolutize(&cwd, p)).collect()
        };

        let mut out = context.stdout();
        let mut exit = 0u8;
        for target in &targets {
            match std::fs::read_dir(target) {
                Ok(entries) => {
                    let mut names: Vec<(String, u64, bool)> = Vec::new();
                    for e in entries.flatten() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if !self.all && name.starts_with('.') {
                            continue;
                        }
                        let md = e.metadata().ok();
                        let size = md.as_ref().map_or(0, |m| m.len());
                        let is_dir = md.as_ref().is_some_and(|m| m.is_dir());
                        names.push((name, size, is_dir));
                    }
                    names.sort();
                    for (name, size, is_dir) in names {
                        let suffix = if is_dir { "/" } else { "" };
                        if self.long {
                            writeln!(out, "{size:>10}  {name}{suffix}")?;
                        } else {
                            writeln!(out, "{name}{suffix}")?;
                        }
                    }
                }
                Err(e) => {
                    if target.is_file() {
                        writeln!(out, "{}", target.display())?;
                    } else {
                        writeln!(context.stderr(), "ls: {}: {e}", target.display())?;
                        exit = 1;
                    }
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    }
}

/// Concatenate files to standard output.
#[derive(Parser)]
pub struct CatCommand {
    files: Vec<String>,
}

impl builtins::Command for CatCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let mut out = context.stdout();
        let mut exit = 0u8;
        if self.files.is_empty() {
            let mut buf = Vec::new();
            context.stdin().read_to_end(&mut buf)?;
            out.write_all(&buf)?;
        } else {
            for f in &self.files {
                match std::fs::read(absolutize(&cwd, f)) {
                    Ok(data) => out.write_all(&data)?,
                    Err(e) => {
                        writeln!(context.stderr(), "cat: {f}: {e}")?;
                        exit = 1;
                    }
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    }
}

/// Count lines, words and bytes.
#[derive(Parser)]
pub struct WcCommand {
    /// Count lines only.
    #[arg(short = 'l')]
    lines_only: bool,
    files: Vec<String>,
}

impl builtins::Command for WcCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let mut out = context.stdout();

        let count = |data: &[u8]| -> (usize, usize, usize) {
            let lines = data.iter().filter(|&&b| b == b'\n').count();
            let words = data
                .split(|b| b.is_ascii_whitespace())
                .filter(|w| !w.is_empty())
                .count();
            (lines, words, data.len())
        };

        if self.files.is_empty() {
            let mut buf = Vec::new();
            context.stdin().read_to_end(&mut buf)?;
            let (l, w, b) = count(&buf);
            if self.lines_only {
                writeln!(out, "{l}")?;
            } else {
                writeln!(out, "{l:>8}{w:>8}{b:>8}")?;
            }
        } else {
            for f in &self.files {
                let data = std::fs::read(absolutize(&cwd, f)).unwrap_or_default();
                let (l, w, b) = count(&data);
                if self.lines_only {
                    writeln!(out, "{l} {f}")?;
                } else {
                    writeln!(out, "{l:>8}{w:>8}{b:>8} {f}")?;
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

/// Output the first lines of input.
#[derive(Parser)]
pub struct HeadCommand {
    /// Number of lines to print.
    #[arg(short = 'n', default_value = "10")]
    count: usize,
    files: Vec<String>,
}

impl builtins::Command for HeadCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let mut out = context.stdout();
        if self.files.is_empty() {
            let reader = BufReader::new(context.stdin());
            for line in reader.lines().take(self.count) {
                writeln!(out, "{}", line?)?;
            }
        } else {
            for f in &self.files {
                let data = std::fs::read_to_string(absolutize(&cwd, f)).unwrap_or_default();
                for line in data.lines().take(self.count) {
                    writeln!(out, "{line}")?;
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

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

/// Print system information.
#[derive(Parser)]
pub struct UnameCommand {
    /// Print all information.
    #[arg(short = 'a')]
    all: bool,
}

impl builtins::Command for UnameCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let mut out = context.stdout();
        if self.all {
            writeln!(
                out,
                "aShell-rs iOS {} brush-core/{} rustc-static",
                std::env::consts::ARCH,
                env!("CARGO_PKG_VERSION")
            )?;
        } else {
            writeln!(out, "aShell-rs")?;
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

/// Create directories.
#[derive(Parser)]
pub struct MkdirCommand {
    /// Create parent directories as needed.
    #[arg(short = 'p')]
    parents: bool,
    dirs: Vec<String>,
}

impl builtins::Command for MkdirCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let mut exit = 0u8;
        for d in &self.dirs {
            let path = absolutize(&cwd, d);
            let result = if self.parents {
                std::fs::create_dir_all(&path)
            } else {
                std::fs::create_dir(&path)
            };
            if let Err(e) = result {
                writeln!(context.stderr(), "mkdir: {d}: {e}")?;
                exit = 1;
            }
        }
        Ok(ExecutionResult::new(exit))
    }
}

/// Remove files or directories.
#[derive(Parser)]
pub struct RmCommand {
    /// Remove directories recursively.
    #[arg(short = 'r')]
    recursive: bool,
    /// Ignore missing files.
    #[arg(short = 'f')]
    force: bool,
    paths: Vec<String>,
}

impl builtins::Command for RmCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let mut exit = 0u8;
        for p in &self.paths {
            let path = absolutize(&cwd, p);
            let result = if path.is_dir() {
                if self.recursive {
                    std::fs::remove_dir_all(&path)
                } else {
                    Err(std::io::Error::other("is a directory"))
                }
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(e) = result {
                if !self.force {
                    writeln!(context.stderr(), "rm: {p}: {e}")?;
                    exit = 1;
                }
            }
        }
        Ok(ExecutionResult::new(exit))
    }
}

/// Create empty files or update timestamps.
#[derive(Parser)]
pub struct TouchCommand {
    files: Vec<String>,
}

impl builtins::Command for TouchCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        for f in &self.files {
            let path = absolutize(&cwd, f);
            if !path.exists() {
                std::fs::File::create(&path)?;
            }
        }
        Ok(ExecutionResult::success())
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
