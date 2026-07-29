//! `grep`, forwarded to uutils/grep.
//!
//! This replaces a hand-written CLI over ripgrep's engine crates. That version
//! was the reason the flag-coverage gate exists: it shipped with 9 flags, and a
//! missing `-E` made a piped `grep` exit before reading its stdin, which killed
//! the app through the resulting broken pipe. It was later filled out to 36
//! flags, but every one of those was a flag we chose to write — the set was
//! bounded by our effort, not by GNU.
//!
//! uutils/grep is a real GNU grep behind the same `uumain(args)` entry point as
//! coreutils and sed, so nothing here parses flags or searches text, and the
//! flag surface is whatever upstream ships (47 long options as adopted).
//!
//! Audited before adoption (see the checklist in `command_host`): the library
//! has no `process::exit`, no `process::Command`, no `static mut` and no signal
//! handling. Its only `exit` lives in `src/main.rs`, which we do not compile.
//! So it needed no vendoring or patching.
//!
//! One consequence worth knowing: upstream backs `-P`/BRE/ERE with oniguruma
//! (C) rather than Rust's `regex`, so backreferences and lookaround now work
//! where the previous adapter had to reject or approximate them.

use std::ffi::OsString;

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: grep (uutils, in-process)"))
}

fn grep_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    // uucore's exit code is a process-global atomic that utilities set on error
    // and never clear; without a reset, one failed `grep` makes every later
    // success report the stale nonzero code.
    uucore::error::set_exit_code(0);
    let mut args: Vec<OsString> = ctx.argv.iter().map(OsString::from).collect();
    // Historical command names carry fixed grep modes. Preserve explicit
    // options by injecting the compatibility mode only when invoked through
    // the alias.
    if ctx.name == "egrep" {
        args.insert(1, OsString::from("-E"));
    } else if ctx.name == "fgrep" {
        args.insert(1, OsString::from("-F"));
    }
    // `#[uucore::main]` already wraps the `UResult<()>` body: it prints the
    // error the way GNU grep would and hands back the exit code.
    uu_grep::uumain(args.into_iter())
}

fn exec_grep(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, grep_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_grep,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
