//! `sed`, forwarded to uutils/sed.
//!
//! This replaces a hand-written adapter over the `sedregex` crate that
//! supported exactly two flags — no `-i`, no `-E`, no `-f`, no `-n`. uutils/sed
//! is a real GNU sed, and its entry point has the same `uumain(args)` shape as
//! coreutils, so nothing here parses flags or edits text.
//!
//! Audited before adoption (see the checklist in `command_host`): the crate's
//! only `process::exit` is in its own `src/bin/sed.rs`, which we do not compile,
//! and its only `Command::new` is `clap::Command` — the argument parser, not a
//! process. So it needed no vendoring or patching.

use std::ffi::OsString;

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
    Ok(format!("{name}: sed (uutils, in-process)"))
}

/// Works around an upstream bug in uutils/sed 0.1.1.
///
/// It declares `-i` as `num_args(0..=1)`, so clap greedily eats the *next*
/// token as the backup suffix: `sed -i 's/a/b/' f.txt` loses its script and
/// fails with "unterminated regular expression". GNU sed only ever accepts an
/// *attached* suffix (`-i.bak`), never a separate token.
///
/// Rewriting a bare `-i` to the unambiguous `--in-place=` form is the smallest
/// fix that does not fork the crate; attached forms (`-i.bak`) are left alone
/// because clap parses those correctly. Remove this once upstream constrains
/// the argument.
fn normalize_in_place(argv: &[String]) -> Vec<OsString> {
    argv.iter()
        .map(|a| {
            if a == "-i" {
                OsString::from("--in-place=")
            } else {
                OsString::from(a)
            }
        })
        .collect()
}

fn sed_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    let args: Vec<OsString> = normalize_in_place(ctx.argv);
    // `#[uucore::main]` already wraps the `UResult<()>` body: it prints the
    // error the way GNU sed would and hands back the exit code.
    sed::sed::uumain(args.into_iter())
}

fn exec_sed(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, sed_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_sed,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
