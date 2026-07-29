//! Registration and CLI-surface contract for every exposed command.
//! Functional semantics live in the 356+ case battery; this test ensures a new
//! upstream command cannot enter the registry without being callable through
//! the exact production shell.

use std::io::Read;

use ashellcore::CommandSource;
use brush_core::openfiles::OpenFile;

async fn run(shell: &mut brush_core::Shell, script: &str) -> (i32, String) {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let output = OpenFile::from(writer);
    shell.open_files_mut().set_fd(1.into(), output.clone());
    shell.open_files_mut().set_fd(2.into(), output);
    let params = shell.default_exec_params();
    let result = shell
        .run_string(
            script.to_string(),
            &brush_core::SourceInfo::from("command-inventory"),
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
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = std::env::temp_dir().join(format!("yourshell_inventory_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut shell = ashellcore::build_shell_for_tests(&dir).await.unwrap();
        let inventory = ashellcore::command_inventory();

        for command in &inventory {
            let (code, output) = run(
                &mut shell,
                &format!("type -- {} >/dev/null 2>&1", command.name),
            )
            .await;
            assert_eq!(
                code, 0,
                "{} is inventoried but not registered: {output}",
                command.name
            );

            // Every uutils command is required to preserve its upstream parser
            // and help path. This catches argv[0] dispatch mistakes and stale
            // registry entries without performing a destructive operation.
            if command.source == CommandSource::UutilsCoreutils {
                let (code, output) =
                    run(&mut shell, &format!("{} --help >/dev/null", command.name)).await;
                assert_eq!(
                    code, 0,
                    "{} rejected its upstream --help path: {output}",
                    command.name
                );
            }
        }
        println!(
            "=== command inventory: {} registered commands verified ===",
            inventory.len()
        );
    });
}
