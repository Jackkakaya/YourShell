//! `tar`, forwarded to libarchive's official bsdtar command-line frontend.
//!
//! Parsing and archive behavior are upstream. This module only constructs a C
//! argv and enters the shared process-shaped command host.

use std::ffi::{c_char, c_int};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    fn ys_bsdtar_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!(
        "{name}: libarchive bsdtar 3.8.8 frontend (in-process)"
    ))
}

fn tar_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    command_host::run_c_argv(ctx, ys_bsdtar_run)
}

fn exec_tar(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, tar_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_tar,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
