//! `diff` and `cmp`, forwarded to uutils/diffutils.
//!
//! This replaces a hand-written `diff` that supported exactly one flag — not
//! even `-u`, the unified format everything downstream expects — and adds `cmp`,
//! which did not exist at all.
//!
//! The crate is vendored (`vendor/patches/diffutils`, MIT/Apache-2.0) because
//! the pre-adoption audit found three blockers, all recorded in the patch
//! comments there:
//!
//! - `diff.rs` called `exit(2)` on a bad flag and `cmp.rs` called `exit(0)` on
//!   `--help`. In a shared host process that terminates the whole app.
//! - Both entry points took `Peekable<ArgsOs>`, and `ArgsOs` can only come from
//!   `std::env::args_os()` — impossible to build from a shell's argv. The
//!   underlying `parse_params` was already generic, so only the signatures
//!   needed widening.
//! - They returned `std::process::ExitCode`, which is opaque, so the exit status
//!   could never reach the caller's `$?`. Changed to `i32`.
//!
//! (Their `Command::new("patch")` calls are all inside `#[cfg(test)]`, which we
//! do not compile — no exec seam needed here.)

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
    Ok(format!("{name}: diffutils (uutils, in-process)"))
}

fn diff_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    let args: Vec<OsString> = ctx.argv.iter().map(OsString::from).collect();
    diffutilslib::diff::main(args.into_iter().peekable())
}

fn cmp_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    let args: Vec<OsString> = ctx.argv.iter().map(OsString::from).collect();
    diffutilslib::cmp::main(args.into_iter().peekable())
}

fn exec_diff(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, diff_main)
}

fn exec_cmp(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, cmp_main)
}

pub fn diff_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_diff,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

pub fn cmp_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_cmp,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
