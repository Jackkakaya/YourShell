//! Integration tests for the agent-facing capture/cancel FFI
//! (`ashell_run_capture`, `ashell_cancel`) driven through the real C ABI.
//!
//! Kept as a SINGLE test: it models the single-session usage the agent path
//! actually has. Running the cancel/timeout cases as separate parallel
//! `#[test]`s would collide, because cancelling a uutils command leaves its
//! process-global `dup2`'d fd open (there is no way to force-stop vendored C
//! mid-run) — which only matters across *concurrent* sessions.

use std::ffi::{c_char, c_void, CStr, CString};

use ashellcore::{
    ashell_cancel, ashell_capture_free, ashell_run_capture, ashell_session_free,
    ashell_session_new, CaptureResult, Session,
};

extern "C" fn out_noop(_ctx: *mut c_void, _bytes: *const u8, _len: usize) {}
extern "C" fn done_noop(_ctx: *mut c_void, _code: i32, _cwd: *const c_char) {}

fn new_session() -> *mut Session {
    let wd = CString::new(std::env::temp_dir().to_string_lossy().into_owned()).unwrap();
    let s = ashell_session_new(out_noop, done_noop, std::ptr::null_mut(), wd.as_ptr());
    assert!(!s.is_null());
    s
}

fn capture(session: *mut Session, cmd: &str, timeout_ms: u64) -> (i32, String, String) {
    let c = CString::new(cmd).unwrap();
    let res = ashell_run_capture(session, c.as_ptr(), timeout_ms);
    assert!(!res.is_null(), "capture returned null for `{cmd}`");
    let r: &CaptureResult = unsafe { &*res };
    let stdout = unsafe { CStr::from_ptr(r.stdout) }
        .to_string_lossy()
        .into_owned();
    let stderr = unsafe { CStr::from_ptr(r.stderr) }
        .to_string_lossy()
        .into_owned();
    let code = r.exit_code;
    ashell_capture_free(res);
    (code, stdout, stderr)
}

#[test]
fn capture_and_cancel_ffi() {
    let s = new_session();

    // 1. stdout/stderr are captured separately, exit code passes through.
    let (code, stdout, stderr) = capture(s, "echo out; echo err >&2; exit 3", 5000);
    assert_eq!(code, 3);
    assert_eq!(stdout.trim(), "out");
    assert_eq!(stderr.trim(), "err");

    // 2. Large output (>a pipe buffer) is fully captured — no deadlock.
    let (code, stdout, _) = capture(
        s,
        "for i in $(seq 4000); do echo LINE-$i-0123456789; done",
        20000,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.lines().count(), 4000);
    assert!(stdout.contains("LINE-4000-"));

    // 3. Pipelines (uutils stages) work and cwd persists across capture calls.
    let (_, out, _) = capture(s, "printf 'b\\na\\nc\\n' | sort | paste -sd, -", 5000);
    assert_eq!(out.trim(), "a,b,c");
    capture(s, "mkdir -p capdir && cd capdir", 5000);
    let (_, out, _) = capture(s, "basename \"$PWD\"", 5000);
    assert_eq!(out.trim(), "capdir");

    // 4. Timeout returns 124 promptly.
    let start = std::time::Instant::now();
    let (code, _, stderr) = capture(s, "sleep 5", 300);
    assert_eq!(code, 124);
    assert!(stderr.contains("timed out"));
    assert!(start.elapsed() < std::time::Duration::from_secs(3));

    // 5. Cancellation from another thread returns 130, session stays usable.
    let sp = s as usize;
    let h = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        ashell_cancel(sp as *mut Session);
    });
    let start = std::time::Instant::now();
    let (code, _, _) = capture(s, "sleep 5", 0);
    assert_eq!(code, 130);
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    h.join().unwrap();
    let (code, out, _) = capture(s, "echo after-cancel", 5000);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "after-cancel");

    ashell_session_free(s);
}
