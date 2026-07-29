//! The uutils/coreutils command set, forwarded through the command host.
//!
//! uutils is a busybox-style multicall crate: every utility is published as
//! `uumain(argv) -> i32` rather than a `main`, which is why ~74 commands come in
//! for the cost of this file. Nothing here parses flags or implements
//! behaviour — the flag surface is whatever uutils ships.

use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::OnceLock;

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

/// Commands whose brush-native builtin should win over the uutils version
/// (shell semantics, shell-state awareness, no serialization cost).
const KEEP_BRUSH: &[&str] = &["echo", "printf", "pwd", "test", "true", "false"];

/// Commands we deliberately do not expose: `yes` streams forever and we have no
/// interrupt delivery for it; `more` is an interactive pager assuming a tty.
const SKIP: &[&str] = &["yes", "more"];

fn registry() -> &'static HashMap<String, fn(Vec<OsString>) -> i32> {
    static REGISTRY: OnceLock<HashMap<String, fn(Vec<OsString>) -> i32>> = OnceLock::new();
    REGISTRY.get_or_init(brush_coreutils_builtins::bundled_commands)
}

/// Names to register, after policy filtering.
pub fn command_names() -> Vec<String> {
    let mut names: Vec<String> = registry()
        .keys()
        .filter(|n| !KEEP_BRUSH.contains(&n.as_str()) && !SKIP.contains(&n.as_str()))
        .cloned()
        .collect();
    names.sort();
    names
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: coreutils command (uutils, in-process)"))
}

/// Dispatches on the invoked name — one registration serves all ~74 utilities.
fn uutils_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    let Some(func) = registry().get(ctx.name).copied() else {
        return 127;
    };
    // uucore's exit code is a process-global atomic that utilities set on error
    // and never clear; without a reset, one failed `ls` makes every later
    // success report the stale nonzero code.
    uucore::error::set_exit_code(0);
    // brush passes the command name as argv[0], matching uumain's convention.
    func(ctx.argv.iter().map(OsString::from).collect())
}

fn stat_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    uucore::error::set_exit_code(0);
    uu_stat::uumain(ctx.argv.iter().map(OsString::from))
}

fn exec_uutils(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, uutils_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_uutils,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn exec_stat(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, stat_main)
}

pub fn stat_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_stat,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
