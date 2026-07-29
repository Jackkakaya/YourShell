//! `sqlite3`, forwarded to SQLite's official command-line shell.
//!
//! The complete CLI parser, dot-command language, output modes and SQL
//! execution behavior live in upstream `shell.c`. This adapter only converts
//! Rust argv into C argv and hands the invocation to `command_host`.

use std::ffi::{c_char, c_int};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    fn ys_sqlite3_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: SQLite 3.46.0 official shell (in-process)"))
}

fn sqlite_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    // Keep rusqlite/libsqlite3-sys reachable so Cargo propagates its native
    // `sqlite3` link directive. shell.c calls that exact engine directly; it
    // does not use rusqlite's higher-level query API.
    let _linked_engine_version = rusqlite::version();

    command_host::run_c_argv(ctx, ys_sqlite3_run)
}

fn exec_sqlite(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, sqlite_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_sqlite,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
