//! BSD gzip/gunzip CLI forwarded through the shared process Host.

use std::ffi::{c_char, c_int};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    fn ys_bsd_gzip_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

fn content(name: &str, _t: ContentType, _o: &ContentOptions) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: BSD gzip CLI (in-process)"))
}

fn gzip_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    command_host::run_c_argv(ctx, ys_bsd_gzip_run)
}

fn exec_gzip(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, gzip_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_gzip,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
