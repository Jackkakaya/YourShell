//! `awk`, forwarded to one-true-awk (Kernighan's awk, vendored C in vendor/awk).
//!
//! The awk core is C compiled into this crate (see build.rs). Its entry point
//! `ys_awk_main(argc, argv)` reads the process-global `stdin` FILE* and writes
//! the process-global `stdout`/`stderr`, so it goes through the command host
//! like every other process-shaped command.
//!
//! Two things the vendored C was patched for, both consequences of living in a
//! long-lived host process rather than a one-shot one:
//!
//! - `ys_awk_main` resets awk's process-global input/stream state on entry (see
//!   main.c / lib.c `ys_awk_reset_io`), so repeated invocations behave like
//!   fresh awk processes. Without that the second `awk` in a session inherits
//!   the first one's parser state — the same failure mode that forced nextvi to
//!   be dropped.
//! - Fatal awk errors longjmp back out instead of calling `exit`, which would
//!   take the whole app down.
//!
//! awk's `system()` and `cmd | getline` / `print | cmd` are stubbed — iOS has
//! no fork/exec.

use std::ffi::{c_char, c_int, CString};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    /// one-true-awk's renamed `main`, provided by vendor/awk/main.c.
    fn ys_awk_main(argc: c_int, argv: *const *const c_char) -> c_int;
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: awk (one-true-awk, in-process)"))
}

fn awk_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    let cstrings: Vec<CString> = ctx
        .argv
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap_or_default())
        .collect();
    let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
    // SAFETY: `ptrs` outlives the call and every pointer comes from a live
    // CString in `cstrings`; awk only reads them.
    unsafe { ys_awk_main(ptrs.len() as c_int, ptrs.as_ptr()) }
}

fn exec_awk(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, awk_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_awk,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
