//! Commands with no upstream to forward to.
//!
//! Everything that has one lives in an adapter module (`grep_adapter`,
//! `sed_adapter`, `uutils_adapter`, …). What is left here is the residue: a
//! command that is pure terminal control, with no implementation anywhere to
//! adopt.

use std::io::Write;

use brush_core::{builtins, ExecutionResult};
use clap::Parser;

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
