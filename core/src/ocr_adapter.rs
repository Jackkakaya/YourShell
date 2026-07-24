//! `ocr` command backed by the app's Vision-framework host (OCRHost.swift).
//! Compiled only with the `vision` cargo feature; the ys_ocr_* symbols
//! resolve when the app links.

use std::ffi::{c_char, CStr, CString};
use std::io::Write;

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

unsafe extern "C" {
    fn ys_ocr_run(path: *const c_char) -> *mut c_char;
    fn ys_ocr_free(s: *mut c_char);
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_ocr,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!(
        "{name}: recognize text in images (Apple Vision, on-device)"
    ))
}

fn exec_ocr(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let files: Vec<String> = args.iter().skip(1).map(ToString::to_string).collect();
        if files.is_empty() {
            writeln!(context.stderr(), "usage: ocr <image>...")?;
            return Ok(ExecutionResult::new(2));
        }

        let cwd = context.shell.working_dir().to_path_buf();
        let mut out = context.stdout();
        let mut exit = 0u8;
        for f in &files {
            let path = if std::path::Path::new(f).is_absolute() {
                std::path::PathBuf::from(f)
            } else {
                cwd.join(f)
            };
            let c_path = CString::new(path.to_string_lossy().into_owned()).unwrap_or_default();
            let result = unsafe { ys_ocr_run(c_path.as_ptr()) };
            if result.is_null() {
                writeln!(context.stderr(), "ocr: {f}: no result")?;
                exit = 1;
                continue;
            }
            let text = unsafe { CStr::from_ptr(result) }
                .to_string_lossy()
                .into_owned();
            unsafe { ys_ocr_free(result) };
            if let Some(err) = text.strip_prefix("ERROR: ") {
                writeln!(context.stderr(), "ocr: {f}: {err}")?;
                exit = 1;
            } else {
                writeln!(out, "{text}")?;
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    })
}
