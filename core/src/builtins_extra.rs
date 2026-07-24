//! Library-backed in-process commands. Policy: no hand-written command
//! implementations — commands come from uutils (see `uutils_adapter`) or
//! ecosystem crates; this module only hosts thin library adapters.
//!
//! `grep` is powered by ripgrep's engine crates (grep-regex line matcher +
//! grep-searcher line-oriented search); this adapter only maps CLI flags to
//! engine options and routes bytes through the shell's fd table.

use std::io::{Read, Write};
use std::path::PathBuf;

use brush_core::{builtins, ExecutionResult};
use clap::Parser;
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::SearcherBuilder;

/// Clear the terminal screen and scrollback. No crate/uutils equivalent —
/// this is pure terminal control (ANSI), like the `clear`/`tput clear` that
/// terminfo would emit for an xterm.
#[derive(Parser)]
pub struct ClearCommand {
    /// Do not clear the scrollback buffer.
    #[arg(short = 'x')]
    keep_scrollback: bool,
}

impl builtins::Command for ClearCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let mut out = context.stdout();
        if self.keep_scrollback {
            // Clear screen + home cursor.
            write!(out, "\x1b[2J\x1b[H")?;
        } else {
            // Clear screen, clear scrollback, home cursor.
            write!(out, "\x1b[3J\x1b[2J\x1b[H")?;
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
    /// Print only names of files with matches.
    #[arg(short = 'l')]
    files_with_matches: bool,
    /// Match fixed strings, not regular expressions.
    #[arg(short = 'F')]
    fixed_strings: bool,
    /// Match only whole words.
    #[arg(short = 'w')]
    word_regexp: bool,
    /// Suppress error messages about missing files.
    #[arg(short = 's')]
    no_messages: bool,
    pattern: String,
    files: Vec<String>,
}

struct LineHit {
    line_number: u64,
    text: String,
}

impl builtins::Command for GrepCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();

        let matcher = match RegexMatcherBuilder::new()
            .case_insensitive(self.ignore_case)
            .word(self.word_regexp)
            .fixed_strings(self.fixed_strings)
            .build(&self.pattern)
        {
            Ok(m) => m,
            Err(e) => {
                writeln!(context.stderr(), "grep: bad pattern: {e}")?;
                return Ok(ExecutionResult::new(2));
            }
        };

        let mut out = context.stdout();
        let mut matched_any = false;
        let mut had_error = false;
        let multi_file = self.files.len() > 1;

        let inputs: Vec<(Option<String>, Option<Vec<u8>>)> = if self.files.is_empty() {
            let mut buf = Vec::new();
            context.stdin().read_to_end(&mut buf)?;
            vec![(None, Some(buf))]
        } else {
            self.files.iter().map(|f| (Some(f.clone()), None)).collect()
        };

        for (fname, stdin_data) in inputs {
            // Collect matching lines through the searcher, then apply the
            // output mode. The searcher owns line splitting and inversion.
            let mut hits: Vec<LineHit> = Vec::new();
            let mut searcher = SearcherBuilder::new()
                .invert_match(self.invert)
                .line_number(true)
                .build();
            let sink = UTF8(|line_number, line| {
                hits.push(LineHit {
                    line_number,
                    text: line.trim_end_matches(['\n', '\r']).to_string(),
                });
                Ok(true)
            });

            let result = match (&fname, stdin_data) {
                (_, Some(data)) => searcher.search_slice(&matcher, &data, sink),
                (Some(f), None) => {
                    let path = absolutize(&cwd, f);
                    searcher.search_path(&matcher, &path, sink)
                }
                (None, None) => unreachable!(),
            };
            if let Err(e) = result {
                if !self.no_messages {
                    writeln!(
                        context.stderr(),
                        "grep: {}: {e}",
                        fname.as_deref().unwrap_or("-")
                    )?;
                }
                had_error = true;
                continue;
            }

            if !hits.is_empty() {
                matched_any = true;
            }

            let label = fname.as_deref();
            if self.files_with_matches {
                if !hits.is_empty() {
                    writeln!(out, "{}", label.unwrap_or("(standard input)"))?;
                }
                continue;
            }
            if self.count {
                match label {
                    Some(f) if multi_file => writeln!(out, "{f}:{}", hits.len())?,
                    _ => writeln!(out, "{}", hits.len())?,
                }
                continue;
            }
            for hit in &hits {
                let mut prefix = String::new();
                if let Some(f) = label {
                    if multi_file {
                        prefix.push_str(f);
                        prefix.push(':');
                    }
                }
                if self.line_numbers {
                    prefix.push_str(&hit.line_number.to_string());
                    prefix.push(':');
                }
                if self.only_matching && !self.invert {
                    let bytes = hit.text.as_bytes();
                    let mut at = 0;
                    while let Ok(Some(m)) = matcher.find_at(bytes, at) {
                        writeln!(out, "{prefix}{}", &hit.text[m.start()..m.end()])?;
                        at = if m.end() > at { m.end() } else { at + 1 };
                        if at >= bytes.len() {
                            break;
                        }
                    }
                } else {
                    writeln!(out, "{prefix}{}", hit.text)?;
                }
            }
        }
        out.flush()?;

        Ok(ExecutionResult::new(if had_error {
            2
        } else if matched_any {
            0
        } else {
            1
        }))
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
