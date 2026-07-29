//! `unzip`, forwarded to libarchive's official bsdunzip frontend.

use crate::command_host;
use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;
use std::ffi::{c_char, c_int};

unsafe extern "C" {
    fn ys_bsdunzip_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

fn content(name: &str, _t: ContentType, _o: &ContentOptions) -> Result<String, brush_core::Error> {
    Ok(format!(
        "{name}: libarchive bsdunzip 3.8.8 frontend (in-process)"
    ))
}

fn unzip_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    command_host::run_c_argv(ctx, ys_bsdunzip_run)
}

fn exec_unzip(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, unzip_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_unzip,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
