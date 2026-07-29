//! End-to-end contract for app-owned commands. Runs as its own process because
//! the Host callback table is intentionally install-once.

use std::ffi::c_void;
use std::io::Read;
use std::sync::Mutex;

use brush_core::openfiles::OpenFile;

static COPIED: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static OPENED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

extern "C" fn copy(bytes: *const u8, len: usize) -> i32 {
    // SAFETY: callback contract supplies `len` readable bytes.
    let value = unsafe { std::slice::from_raw_parts(bytes, len) };
    *COPIED.lock().unwrap() = value.to_vec();
    0
}

extern "C" fn paste(ctx: *mut c_void, out: extern "C" fn(*mut c_void, *const u8, usize)) -> i32 {
    let value = b"from-clipboard";
    out(ctx, value.as_ptr(), value.len());
    0
}

extern "C" fn open(bytes: *const u8, len: usize) -> i32 {
    // SAFETY: callback contract supplies `len` readable bytes.
    let value = unsafe { std::slice::from_raw_parts(bytes, len) };
    *OPENED.lock().unwrap() = value.to_vec();
    0
}

async fn run(shell: &mut brush_core::Shell, script: &str) -> (i32, String) {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let output = OpenFile::from(writer);
    shell.open_files_mut().set_fd(1.into(), output.clone());
    shell.open_files_mut().set_fd(2.into(), output);
    let params = shell.default_exec_params();
    let result = shell
        .run_string(
            script.to_string(),
            &brush_core::SourceInfo::from("ios-host-test"),
            &params,
        )
        .await;
    shell
        .open_files_mut()
        .set_fd(1.into(), brush_core::openfiles::null().unwrap());
    shell
        .open_files_mut()
        .set_fd(2.into(), brush_core::openfiles::null().unwrap());
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    (
        result.map_or(127, |r| i32::from(u8::from(r.exit_code))),
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn main() {
    assert_eq!(
        ashellcore::ashell_ios_host_install(Some(copy), Some(paste), Some(open)),
        1
    );
    // Installation is immutable so a second component cannot hijack Host I/O.
    assert_eq!(
        ashellcore::ashell_ios_host_install(Some(copy), Some(paste), Some(open)),
        0
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = std::env::temp_dir().join(format!("yourshell_ios_host_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut shell = ashellcore::build_shell_for_tests(&dir).await.unwrap();

        let (code, output) = run(
            &mut shell,
            "printf copied | pbcopy; pbpaste; open https://example.test/a; openurl local.txt",
        )
        .await;
        assert_eq!(code, 0, "{output}");
        assert_eq!(output, "from-clipboard");
        assert_eq!(&*COPIED.lock().unwrap(), b"copied");
        assert_eq!(&*OPENED.lock().unwrap(), b"local.txt");

        let (code, output) = run(&mut shell, "open").await;
        assert_eq!(code, 2);
        assert!(output.contains("missing URL or path"));
    });
    println!("=== iOS Host commands: copy/paste/open/openurl passed ===");
}
