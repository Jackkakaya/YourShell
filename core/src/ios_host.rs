//! Optional app-host capabilities used by iOS-only commands.
//!
//! The core library stays UIKit-free. The app registers these callbacks once
//! during startup; command adapters can then use the same ABI from Rust tests,
//! the iOS static library, and future non-Swift hosts.

use std::ffi::c_void;
use std::sync::OnceLock;

pub type CopyFn = extern "C" fn(*const u8, usize) -> i32;
pub type PasteFn = extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, *const u8, usize)) -> i32;
pub type OpenFn = extern "C" fn(*const u8, usize) -> i32;

#[derive(Clone, Copy)]
struct Host {
    copy: Option<CopyFn>,
    paste: Option<PasteFn>,
    open: Option<OpenFn>,
}

static HOST: OnceLock<Host> = OnceLock::new();

pub fn install(copy: Option<CopyFn>, paste: Option<PasteFn>, open: Option<OpenFn>) -> bool {
    HOST.set(Host { copy, paste, open }).is_ok()
}

pub fn copy(bytes: &[u8]) -> Option<i32> {
    HOST.get()
        .and_then(|h| h.copy)
        .map(|f| f(bytes.as_ptr(), bytes.len()))
}

pub fn paste(ctx: *mut c_void, out: extern "C" fn(*mut c_void, *const u8, usize)) -> Option<i32> {
    HOST.get().and_then(|h| h.paste).map(|f| f(ctx, out))
}

pub fn open(bytes: &[u8]) -> Option<i32> {
    HOST.get()
        .and_then(|h| h.open)
        .map(|f| f(bytes.as_ptr(), bytes.len()))
}
