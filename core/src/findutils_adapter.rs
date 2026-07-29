//! `find` and `xargs`, forwarded to uutils/findutils.
//!
//! Both come from the vendored `findutils` crate (`vendor/patches/findutils`,
//! MIT). Nothing here parses flags or implements behaviour — `find` alone is
//! ~75 GNU predicates, and a partial version of it fails an agent caller in the
//! worst way: it reads as "I got the syntax wrong", so the caller retries
//! variations that also fail.
//!
//! The one thing that had to be adapted is fork/exec. `xargs` exists purely to
//! run commands and `find -exec` is one of its most-used predicates, but iOS
//! forbids process creation. The vendored crate therefore calls
//! `exec_hook::run` instead of `std::process::Command`, and this module
//! installs a hook that runs the argv in a fresh in-process subshell — which is
//! also the semantically correct target, since upstream spawns an independent
//! process and no state should leak back into the caller's shell.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

/// Context the exec hook needs. `find`/`xargs` run synchronously on the
/// blocking thread that sets this up, so a thread-local avoids a global the
/// hook would otherwise have to lock.
struct ExecCtx {
    runtime: tokio::runtime::Runtime,
    cwd: PathBuf,
}

thread_local! {
    static EXEC_CTX: RefCell<Option<ExecCtx>> = const { RefCell::new(None) };
}

/// Quotes one argument for the subshell. Single quotes are literal in POSIX
/// shells except for the quote itself, so `'` becomes `'\''`.
fn shell_quote(arg: &OsString) -> String {
    format!("'{}'", arg.to_string_lossy().replace('\'', r"'\''"))
}

/// Runs `argv` in a fresh subshell — the hook the vendored crate calls in place
/// of `std::process::Command`.
fn exec_in_subshell(
    argv: &[OsString],
    _env: &[(OsString, OsString)],
    cwd: Option<&Path>,
    _close_stdin: bool,
) -> i32 {
    if argv.is_empty() {
        return 127;
    }
    EXEC_CTX.with(|slot| {
        let borrowed = slot.borrow();
        let Some(ctx) = borrowed.as_ref() else {
            // Not inside a find/xargs invocation: nothing sane to run this in.
            return 127;
        };
        let dir = cwd.unwrap_or(&ctx.cwd).to_path_buf();
        let cmdline = argv.iter().map(shell_quote).collect::<Vec<_>>().join(" ");
        ctx.runtime.block_on(async move {
            // Empty fd map: the command host already dup2'd the session's fds
            // onto the process ones, and a shell with no explicit fds inherits
            // those, so the child's output lands where the caller expects.
            let Ok(mut shell) = crate::build_shell(HashMap::new(), &dir).await else {
                return 127;
            };
            let params = shell.default_exec_params();
            let source_info = brush_core::SourceInfo::from("xargs");
            match shell.run_string(cmdline, &source_info, &params).await {
                Ok(result) => i32::from(u8::from(result.exit_code)),
                Err(_) => 127,
            }
        })
    })
}

/// Installs the exec hook and the per-invocation runtime the hook drives.
/// Called from inside the command host's locked section, so the thread-local is
/// visible to the synchronous findutils code that runs next.
fn with_exec_ctx<T>(cwd: &Path, f: impl FnOnce() -> T) -> T {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| findutils::exec_hook::set_exec_hook(exec_in_subshell));

    let ctx = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .map(|runtime| ExecCtx {
            runtime,
            cwd: cwd.to_path_buf(),
        });
    EXEC_CTX.with(|slot| *slot.borrow_mut() = ctx);
    let out = f();
    EXEC_CTX.with(|slot| *slot.borrow_mut() = None);
    out
}

fn content(name: &str, _t: ContentType, _o: &ContentOptions) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: uutils/findutils (in-process)"))
}

fn find_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    with_exec_ctx(ctx.cwd, || {
        let refs: Vec<&str> = ctx.argv.iter().map(String::as_str).collect();
        // StandardDependencies writes to process stdout, which the host has
        // already pointed at the session's fd 1.
        let deps = findutils::find::StandardDependencies::new();
        findutils::find::find_main(&refs, &deps)
    })
}

fn xargs_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    with_exec_ctx(ctx.cwd, || {
        let refs: Vec<&str> = ctx.argv.iter().map(String::as_str).collect();
        findutils::xargs::xargs_main(&refs)
    })
}

// brush takes a bare `fn` for `execute_func`, so the per-command entry cannot be
// captured in a closure — hence one tiny shim each.
fn exec_find(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, find_main)
}

fn exec_xargs(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, xargs_main)
}

pub fn find_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_find,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

pub fn xargs_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_xargs,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
