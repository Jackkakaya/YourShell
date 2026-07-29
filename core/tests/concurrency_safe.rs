//! Smoke test for commands that use Session/Host I/O directly.
//! This intentionally does not use uutils/process-shaped commands: those are
//! expected to serialize on `process_state_lock`.

use std::io::Read;
use std::sync::{Arc, Barrier};

use brush_core::openfiles::OpenFile;

fn main() {
    let base = std::env::temp_dir().join(format!("yourshell_safe_{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|id| {
            let barrier = Arc::clone(&barrier);
            let dir = base.join(format!("s{id}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let mut shell = ashellcore::build_shell_for_tests(&dir).await.unwrap();
                    barrier.wait();
                    let (mut reader, writer) = std::io::pipe().unwrap();
                    let out = OpenFile::from(writer);
                    shell.open_files_mut().set_fd(1.into(), out.clone());
                    shell.open_files_mut().set_fd(2.into(), out);
                    let params = shell.default_exec_params();
                    let result = shell
                        .run_string(
                            "git --version".to_string(),
                            &brush_core::SourceInfo::from("safe-concurrency"),
                            &params,
                        )
                        .await
                        .unwrap();
                    shell
                        .open_files_mut()
                        .set_fd(1.into(), brush_core::openfiles::null().unwrap());
                    shell
                        .open_files_mut()
                        .set_fd(2.into(), brush_core::openfiles::null().unwrap());
                    let mut bytes = Vec::new();
                    reader.read_to_end(&mut bytes).unwrap();
                    assert_eq!(u8::from(result.exit_code), 0, "session {id} failed");
                    let output = String::from_utf8_lossy(&bytes);
                    assert!(output.contains("git ("), "session {id}: {output:?}");
                });
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    println!("=== safe concurrency: 4 Session-safe git commands ===");
}
