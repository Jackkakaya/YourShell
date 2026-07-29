//! `rg`, forwarded to ripgrep's official CLI core.

use std::process::ExitCode;

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
    Ok(format!("{name}: official ripgrep 15.1.0 CLI (in-process)"))
}

fn ripgrep_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    // Upstream expects the iterator after argv[0], exactly as its former
    // std::env::args_os().skip(1) did.
    let code = ripgrep_cli::run_with_stdin_is_interactive(
        ctx.argv.iter().skip(1),
        ctx.stdin_is_interactive,
    );
    if code == ExitCode::SUCCESS {
        0
    } else if code == ExitCode::from(1) {
        1
    } else {
        2
    }
}

fn exec_rg(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, ripgrep_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_rg,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
