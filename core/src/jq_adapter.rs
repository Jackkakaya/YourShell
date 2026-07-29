//! `jq`, forwarded to jq itself (jqlang/jq 1.8.2, MIT, vendored C).
//!
//! This replaces a hand-written CLI over the `jaq` crate. The reason to switch
//! is not flag count — the old adapter passed the flag audit at 11/11. It is
//! that jq is a *language*, so the parts most likely to diverge from upstream
//! fail **silently**: an alternative operator, a path expression, `@base64`,
//! how errors propagate through `try`/`//`, whether `first`/`limit` actually
//! short-circuit. A wrong flag makes a caller change approach; a filter that
//! quietly returns different JSON does not, and a caller writing filters from
//! jq's own manual has no way to tell.
//!
//! The C is built by `build.rs` (see `build_jq` there for the vendoring and
//! pre-generated-file details) and entered through `ys_jq_main`, jq's own
//! `main` renamed. So nothing here parses flags or evaluates filters.
//!
//! Audit findings and the patches they required, all in
//! `vendor/jq/src/main.c`:
//!
//! - **No fork/exec, no signal handling, no file-scope mutable state in
//!   `main.c`** — jq keeps its per-run state in `jq_state`, because it is also
//!   shipped as `libjq`. That is what makes this adoption cheap.
//! - `main` ended by calling `exit()` (directly and through the
//!   `jq_exit`/`jq_exit_with_status` macros), and `usage()`/`die()` exited on
//!   `--help` and on any bad option. Those became `return`s that route through
//!   the `out:` label main already uses, so the normal teardown still runs.
//! - `main` called **`fclose(stdout)`** on the way out. Upstream can: the
//!   process is ending, and closing is how you observe a deferred write error.
//!   Here fd 1 is the caller's pipe and the process outlives the command, so
//!   that would break every command that ran afterwards. Now `fflush`, plus a
//!   `clearerr` on stdout/stdin so a sticky error flag from one run cannot make
//!   the next one report a failure it did not have.
//!
//! The remaining `abort()`s are jq's out-of-memory paths in `jv_alloc.c`, which
//! `catch_unwind` cannot contain either way; they are what any allocator does
//! when it cannot allocate.

use std::ffi::{c_char, c_int, CString};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    /// jq's `main`, renamed. See `vendor/jq/src/main.c`.
    fn ys_jq_main(argc: c_int, argv: *const *mut c_char) -> c_int;
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: jq 1.8.2 (jqlang, in-process)"))
}

fn jq_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    // jq writes `argv` through in its option loop (it rewrites short-option
    // clusters in place), so the pointers must be to writable memory — hence
    // `into_raw` rather than borrowing the CStrings.
    let mut owned: Vec<*mut c_char> = ctx
        .argv
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap_or_default().into_raw())
        .collect();
    // jq indexes argv[0] for JQ_ORIGIN; brush already passes the command name
    // there, matching what a real `main` receives.
    let code = unsafe { ys_jq_main(owned.len() as c_int, owned.as_ptr()) };
    for p in owned.drain(..) {
        drop(unsafe { CString::from_raw(p) });
    }
    code
}

fn exec_jq(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, jq_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_jq,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
