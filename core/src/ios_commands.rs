//! iOS app integrations. Parsing remains in Brush; UIKit work stays in Host.

use std::ffi::c_void;
use std::io::{Read, Write};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::ios_host;

fn content(name: &str, _: ContentType, _: &ContentOptions) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: iOS app host command"))
}

extern "C" fn write_stdout(ctx: *mut c_void, bytes: *const u8, len: usize) {
    if ctx.is_null() || bytes.is_null() {
        return;
    }
    // SAFETY: the Host callback is synchronous and receives the context passed
    // by paste_main for the duration of this call.
    let out = unsafe { &mut *(ctx as *mut Vec<u8>) };
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, len) });
}

fn exec_copy(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    _args: Vec<brush_core::CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let mut input = Vec::new();
        context.stdin().read_to_end(&mut input)?;
        let code = ios_host::copy(&input).unwrap_or_else(|| {
            let _ = writeln!(context.stderr(), "pbcopy: iOS Host is not installed");
            127
        });
        Ok(ExecutionResult::new(code as u8))
    })
}

fn exec_paste(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    _args: Vec<brush_core::CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let mut bytes = Vec::new();
        let code = ios_host::paste(&mut bytes as *mut _ as *mut c_void, write_stdout)
            .unwrap_or_else(|| {
                let _ = writeln!(context.stderr(), "pbpaste: iOS Host is not installed");
                127
            });
        if code == 0 {
            context.stdout().write_all(&bytes)?;
        }
        Ok(ExecutionResult::new(code as u8))
    })
}

pub fn copy_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_copy,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
pub fn paste_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_paste,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn exec_open(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<brush_core::CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let values: Vec<String> = args.iter().map(ToString::to_string).collect();
        let Some(target) = values.get(1) else {
            let _ = writeln!(context.stderr(), "open: missing URL or path");
            return Ok(ExecutionResult::new(2));
        };
        let code = ios_host::open(target.as_bytes()).unwrap_or_else(|| {
            let _ = writeln!(context.stderr(), "open: iOS Host is not installed");
            127
        });
        Ok(ExecutionResult::new(code as u8))
    })
}

pub fn open_registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_open,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}
