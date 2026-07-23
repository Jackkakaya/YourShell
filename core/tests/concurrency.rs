//! Multi-session isolation under concurrency (harness = false, see battery.rs).
//!
//! Four sessions run simultaneously on their own threads, each with its own
//! Shell in its own scratch dir. Verifies: per-session cwd/env isolation, and
//! that the uutils adapter's global-fd serialization never leaks one
//! session's output or state into another.

use std::io::Read;

use brush_core::openfiles::OpenFile;

fn run_session(id: usize, base: &std::path::Path) -> Result<(), String> {
    let dir = base.join(format!("sess{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    runtime.block_on(async {
        let mut shell = ashellcore::build_shell_for_tests(&dir)
            .await
            .map_err(|e| e.to_string())?;

        for round in 0..25 {
            let marker = format!("s{id}r{round}");
            let script = format!(
                "export SESSVAR={marker}; \
                 echo {marker} > data_{marker}.txt; \
                 mkdir -p sub_{marker} && cd sub_{marker} && pwd && cd ..; \
                 cat data_{marker}.txt; \
                 printf '{marker}\n{marker}\n' | sort | uniq -c | tr -s ' '; \
                 printf {marker} | sha256sum | head -c 8; echo; \
                 printenv SESSVAR"
            );
            let (exit, out) = run_capture(&mut shell, &script).await;
            if exit != 0 {
                return Err(format!("session {id} round {round}: exit {exit}, out={out:?}"));
            }
            // cwd isolation: pwd must point into THIS session's dir.
            if !out.contains(&format!("sess{id}/sub_{marker}")) {
                return Err(format!("session {id} round {round}: cwd leak, out={out:?}"));
            }
            // env + data isolation: our marker present, no other session's.
            if !out.contains(&marker) {
                return Err(format!("session {id} round {round}: marker missing, out={out:?}"));
            }
            for other in 0..4 {
                if other != id && out.contains(&format!("s{other}r")) {
                    return Err(format!("session {id} round {round}: cross-talk from {other}, out={out:?}"));
                }
            }
            // uutils output sanity: "2 marker" from uniq -c.
            if !out.contains(&format!("2 {marker}")) {
                return Err(format!("session {id} round {round}: uniq output wrong, out={out:?}"));
            }
        }
        Ok(())
    })
}

async fn run_capture(shell: &mut brush_core::Shell, script: &str) -> (i32, String) {
    let (mut reader, writer) = std::io::pipe().expect("pipe");
    let out = OpenFile::from(writer);
    shell.open_files_mut().set_fd(1.into(), out.clone());
    shell.open_files_mut().set_fd(2.into(), out);

    let params = shell.default_exec_params();
    let source_info = brush_core::SourceInfo::from("concurrency");
    let result = shell.run_string(script.to_string(), &source_info, &params).await;
    let exit = match result {
        Ok(r) => i32::from(u8::from(r.exit_code)),
        Err(_) => 127,
    };

    shell
        .open_files_mut()
        .set_fd(1.into(), brush_core::openfiles::null().expect("null"));
    shell
        .open_files_mut()
        .set_fd(2.into(), brush_core::openfiles::null().expect("null"));

    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    (exit, String::from_utf8_lossy(&buf).into_owned())
}

fn main() {
    let base = std::env::temp_dir().join(format!("yourshell_conc_{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();

    let handles: Vec<_> = (0..4)
        .map(|id| {
            let base = base.clone();
            std::thread::spawn(move || run_session(id, &base))
        })
        .collect();

    let mut failed = false;
    for (id, h) in handles.into_iter().enumerate() {
        match h.join() {
            Ok(Ok(())) => println!("PASS session {id} (25 rounds)"),
            Ok(Err(e)) => {
                println!("FAIL {e}");
                failed = true;
            }
            Err(_) => {
                println!("FAIL session {id} panicked");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    println!("=== concurrency: 4 sessions x 25 rounds isolated ===");
}
