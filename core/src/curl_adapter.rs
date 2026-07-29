//! curl 8.1.2's official command-line frontend, compiled in-process.
//!
//! All argv parsing, HTTP behavior, cookies, proxy handling, retries, config
//! files and output formatting live in upstream curl. This adapter only
//! converts argv and enters the shared session/process Host.

use std::ffi::{c_char, c_int};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    fn ys_curl_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: curl 8.1.2 official CLI (in-process)"))
}

fn curl_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    command_host::run_c_argv(ctx, ys_curl_run)
}

fn exec(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, curl_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
