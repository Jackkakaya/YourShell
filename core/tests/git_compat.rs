//! Differential repository-state tests against the desktop Git CLI.
//!
//! Output wording is deliberately not compared yet: repository state and exit
//! status are the first compatibility contract. Each scenario is executed in
//! two independent worktrees, one by desktop Git and one by YourShell.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use brush_core::openfiles::OpenFile;

fn desktop_git(cwd: &Path, args: &[&str]) -> (i32, String) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
        .expect("run desktop git");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(128), text)
}

async fn shell_git(shell: &mut brush_core::Shell, script: &str) -> (i32, String) {
    let (mut reader, writer) = std::io::pipe().expect("pipe");
    let out = OpenFile::from(writer);
    shell.open_files_mut().set_fd(1.into(), out.clone());
    shell.open_files_mut().set_fd(2.into(), out);
    let params = shell.default_exec_params();
    let result = shell
        .run_string(
            script.to_string(),
            &brush_core::SourceInfo::from("git-compat"),
            &params,
        )
        .await;
    let code = result.map_or(127, |r| i32::from(u8::from(r.exit_code)));
    shell
        .open_files_mut()
        .set_fd(1.into(), brush_core::openfiles::null().expect("null"));
    shell
        .open_files_mut()
        .set_fd(2.into(), brush_core::openfiles::null().expect("null"));
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).expect("read output");
    (code, String::from_utf8_lossy(&bytes).into_owned())
}

fn desktop_setup(repo: &Path) {
    assert_eq!(desktop_git(repo, &["init", "-q"]).0, 0);
    assert_eq!(
        desktop_git(repo, &["config", "user.name", "YourShell Test"]).0,
        0
    );
    assert_eq!(
        desktop_git(repo, &["config", "user.email", "test@yourshell.local"]).0,
        0
    );
}

fn repository_snapshot(repo: &Path) -> String {
    let commands: &[&[&str]] = &[
        &["rev-parse", "HEAD^{tree}"],
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &["branch", "--format=%(refname:short)"],
        &["tag", "--list"],
        &["ls-tree", "-r", "--name-only", "HEAD"],
    ];
    let mut result = String::new();
    for args in commands {
        let (code, output) = desktop_git(repo, args);
        result.push_str(&format!("{} [{code}]\n{output}", args.join(" ")));
    }
    result
}

fn rebase_state_exists(repo: &Path) -> bool {
    let git_dir = repo.join(".git");
    git_dir.join("rebase-merge").exists()
        || git_dir.join("rebase-apply").exists()
        || git_dir.join("YOURSHELL_REBASE_AUTOSTASH").exists()
}

async fn replay_conflict_case(
    shell: &mut brush_core::Shell,
    base: &Path,
    command: &str,
    should_continue: bool,
) -> bool {
    let suffix = if should_continue { "continue" } else { "abort" };
    let oracle = base.join(format!("oracle-{command}-{suffix}"));
    let actual = base.join(format!("actual-{command}-{suffix}"));
    std::fs::create_dir(&oracle).expect("oracle replay dir");
    std::fs::create_dir(&actual).expect("actual replay dir");
    desktop_setup(&oracle);
    std::fs::write(oracle.join("conflict.txt"), "base\n").expect("base");
    assert_eq!(desktop_git(&oracle, &["add", "conflict.txt"]).0, 0);
    assert_eq!(desktop_git(&oracle, &["commit", "-q", "-m", "base"]).0, 0);
    if command == "cherry-pick" {
        assert_eq!(desktop_git(&oracle, &["switch", "-q", "-c", "source"]).0, 0);
        std::fs::write(oracle.join("conflict.txt"), "source\n").expect("source");
        assert_eq!(
            desktop_git(&oracle, &["commit", "-q", "-am", "source"]).0,
            0
        );
        assert_eq!(desktop_git(&oracle, &["switch", "-q", "master"]).0, 0);
    } else {
        std::fs::write(oracle.join("conflict.txt"), "target\n").expect("target");
        assert_eq!(
            desktop_git(&oracle, &["commit", "-q", "-am", "target"]).0,
            0
        );
        assert_eq!(desktop_git(&oracle, &["tag", "target"]).0, 0);
    }
    std::fs::write(oracle.join("conflict.txt"), "current\n").expect("current");
    assert_eq!(
        desktop_git(&oracle, &["commit", "-q", "-am", "current"]).0,
        0
    );
    let target = if command == "cherry-pick" {
        "source"
    } else {
        "target"
    };
    assert_ne!(desktop_git(&oracle, &[command, target]).0, 0);
    assert!(desktop_git(&oracle, &["status", "--porcelain"])
        .1
        .contains("UU conflict.txt"));
    if should_continue {
        std::fs::write(oracle.join("conflict.txt"), "resolved\n").expect("resolve");
        assert_eq!(desktop_git(&oracle, &["add", "conflict.txt"]).0, 0);
        assert_eq!(desktop_git(&oracle, &[command, "--continue"]).0, 0);
    } else {
        assert_eq!(desktop_git(&oracle, &[command, "--abort"]).0, 0);
    }

    let actual_text = actual.to_string_lossy();
    let setup = if command == "cherry-pick" {
        format!(
            "cd {actual_text}; git init -q; git config user.name 'YourShell Test'; \
             git config user.email test@yourshell.local; printf 'base\\n' > conflict.txt; \
             git add conflict.txt; git commit -m base; git switch -c source; \
             printf 'source\\n' > conflict.txt; git add conflict.txt; git commit -m source; \
             git switch master; printf 'current\\n' > conflict.txt; git add conflict.txt; \
             git commit -m current"
        )
    } else {
        format!(
            "cd {actual_text}; git init -q; git config user.name 'YourShell Test'; \
             git config user.email test@yourshell.local; printf 'base\\n' > conflict.txt; \
             git add conflict.txt; git commit -m base; printf 'target\\n' > conflict.txt; \
             git add conflict.txt; git commit -m target; git tag target; \
             printf 'current\\n' > conflict.txt; git add conflict.txt; git commit -m current"
        )
    };
    let finish = if should_continue {
        format!(
            "git {command} {target}; test $? -ne 0 && \
             git status --porcelain | grep '^UU conflict.txt' >/dev/null && \
             printf 'resolved\\n' > conflict.txt && git add conflict.txt && \
             git {command} --continue"
        )
    } else {
        format!(
            "git {command} {target}; test $? -ne 0 && \
             git status --porcelain | grep '^UU conflict.txt' >/dev/null && \
             git {command} --abort"
        )
    };
    let (code, output) = shell_git(shell, &format!("{setup}; {finish}")).await;
    if code != 0 {
        eprintln!("FAIL {command} conflict {suffix}: exit {code}\n{output}");
        return false;
    }
    let expected = repository_snapshot(&oracle);
    let got = repository_snapshot(&actual);
    let expected_file = std::fs::read(oracle.join("conflict.txt")).ok();
    let got_file = std::fs::read(actual.join("conflict.txt")).ok();
    if expected != got || expected_file != got_file {
        eprintln!(
            "FAIL {command} conflict {suffix} differs\n--- desktop ---\n{expected}\n--- YourShell ---\n{got}\n{output}"
        );
        return false;
    }
    println!("PASS {command} conflict {suffix}");
    true
}

async fn rebase_conflict_case(
    shell: &mut brush_core::Shell,
    base: &Path,
    should_continue: bool,
) -> bool {
    let suffix = if should_continue { "continue" } else { "abort" };
    let oracle = base.join(format!("oracle-rebase-{suffix}"));
    let actual = base.join(format!("actual-rebase-{suffix}"));
    std::fs::create_dir(&oracle).expect("oracle rebase dir");
    std::fs::create_dir(&actual).expect("actual rebase dir");
    desktop_setup(&oracle);
    std::fs::write(oracle.join("conflict.txt"), "base\n").expect("base");
    for args in [
        &["add", "conflict.txt"][..],
        &["commit", "-q", "-m", "base"][..],
        &["switch", "-q", "-c", "topic"][..],
    ] {
        assert_eq!(desktop_git(&oracle, args).0, 0);
    }
    std::fs::write(oracle.join("conflict.txt"), "topic\n").expect("topic");
    assert_eq!(desktop_git(&oracle, &["commit", "-q", "-am", "topic"]).0, 0);
    assert_eq!(desktop_git(&oracle, &["switch", "-q", "master"]).0, 0);
    std::fs::write(oracle.join("conflict.txt"), "master\n").expect("master");
    assert_eq!(
        desktop_git(&oracle, &["commit", "-q", "-am", "master"]).0,
        0
    );
    assert_eq!(desktop_git(&oracle, &["switch", "-q", "topic"]).0, 0);
    assert_ne!(desktop_git(&oracle, &["rebase", "master"]).0, 0);
    assert!(desktop_git(&oracle, &["status", "--porcelain"])
        .1
        .contains("UU conflict.txt"));
    if should_continue {
        std::fs::write(oracle.join("conflict.txt"), "resolved\n").expect("resolve");
        assert_eq!(desktop_git(&oracle, &["add", "conflict.txt"]).0, 0);
        assert_eq!(desktop_git(&oracle, &["rebase", "--continue"]).0, 0);
    } else {
        assert_eq!(desktop_git(&oracle, &["rebase", "--abort"]).0, 0);
    }

    let actual_text = actual.to_string_lossy();
    let action = if should_continue {
        "printf 'resolved\\n' > conflict.txt && git add conflict.txt && git rebase --continue"
    } else {
        "git rebase --abort"
    };
    let script = format!(
        "cd {actual_text}; git init -q; git config user.name 'YourShell Test'; \
         git config user.email test@yourshell.local; printf 'base\\n' > conflict.txt; \
         git add conflict.txt; git commit -m base; git switch -c topic; \
         printf 'topic\\n' > conflict.txt; git add conflict.txt; git commit -m topic; \
         git switch master; printf 'master\\n' > conflict.txt; git add conflict.txt; \
         git commit -m master; git switch topic; git rebase master; test $? -ne 0 && \
         git status --porcelain | grep '^UU conflict.txt' >/dev/null && {action}"
    );
    let (code, output) = shell_git(shell, &script).await;
    if code != 0 {
        eprintln!("FAIL rebase conflict {suffix}: exit {code}\n{output}");
        return false;
    }
    let expected = repository_snapshot(&oracle);
    let got = repository_snapshot(&actual);
    if expected != got
        || std::fs::read(oracle.join("conflict.txt")).ok()
            != std::fs::read(actual.join("conflict.txt")).ok()
    {
        eprintln!(
            "FAIL rebase conflict {suffix} differs\n--- desktop ---\n{expected}\n--- YourShell ---\n{got}\n{output}"
        );
        return false;
    }
    println!("PASS rebase conflict {suffix}");
    true
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let ok = runtime.block_on(async {
        let base = std::env::temp_dir().join(format!("yourshell_git_{}", std::process::id()));
        let oracle = base.join("oracle");
        let actual = base.join("actual");
        std::fs::create_dir_all(&oracle).expect("oracle dir");
        std::fs::create_dir_all(&actual).expect("actual dir");

        desktop_setup(&oracle);
        std::fs::write(oracle.join("one.txt"), "one\n").expect("oracle file");
        std::fs::create_dir(oracle.join("dir")).expect("oracle subdir");
        std::fs::write(oracle.join("dir/two.txt"), "two\n").expect("oracle file");
        for args in [
            &["add", "."][..],
            &["commit", "-q", "-m", "initial"][..],
            &["branch", "topic"][..],
            &["switch", "-q", "-c", "feature"][..],
        ] {
            let (code, output) = desktop_git(&oracle, args);
            assert_eq!(code, 0, "desktop git {}: {output}", args.join(" "));
        }
        std::fs::write(oracle.join("move.txt"), "move\n").expect("oracle move file");
        for args in [
            &["add", "move.txt"][..],
            &["commit", "-q", "-m", "second"][..],
            &["tag", "v1"][..],
            &["mv", "move.txt", "moved.txt"][..],
            &["rm", "-q", "dir/two.txt"][..],
            &["commit", "-q", "-m", "rename and remove"][..],
            &["reset", "--hard", "HEAD"][..],
            &["show", "--quiet", "HEAD"][..],
            &["switch", "-q", "master"][..],
        ] {
            let (code, output) = desktop_git(&oracle, args);
            assert_eq!(code, 0, "desktop git {}: {output}", args.join(" "));
        }
        std::fs::write(oracle.join("master.txt"), "master\n").expect("oracle master file");
        for args in [
            &["add", "master.txt"][..],
            &["commit", "-q", "-m", "master work"][..],
            &["merge", "-q", "--no-edit", "feature"][..],
            &["switch", "-q", "-c", "cherry-source"][..],
        ] {
            let (code, output) = desktop_git(&oracle, args);
            assert_eq!(code, 0, "desktop git {}: {output}", args.join(" "));
        }
        std::fs::write(oracle.join("cherry.txt"), "cherry\n").expect("oracle cherry file");
        for args in [
            &["add", "cherry.txt"][..],
            &["commit", "-q", "-m", "cherry work"][..],
            &["switch", "-q", "master"][..],
            &["cherry-pick", "cherry-source"][..],
            &["revert", "--no-edit", "HEAD"][..],
        ] {
            let (code, output) = desktop_git(&oracle, args);
            assert_eq!(code, 0, "desktop git {}: {output}", args.join(" "));
        }

        let mut shell = ashellcore::build_shell_for_tests(&actual)
            .await
            .expect("shell");
        let script = "git init -q; \
                      git config user.name 'YourShell Test'; \
                      git config user.email test@yourshell.local; \
                      printf 'one\\n' > one.txt; mkdir dir; printf 'two\\n' > dir/two.txt; \
                      git add .; git commit -m initial; git branch topic; git switch -c feature; \
                      printf 'move\\n' > move.txt; git add move.txt; git commit -m second; \
                      git tag v1; git mv move.txt moved.txt; git rm dir/two.txt; \
                      git commit -m 'rename and remove'; git reset --hard HEAD; \
                      git show HEAD >/dev/null; git switch master; \
                      printf 'master\\n' > master.txt; git add master.txt; \
                      git commit -m 'master work'; git merge feature; \
                      git switch -c cherry-source; printf 'cherry\\n' > cherry.txt; \
                      git add cherry.txt; git commit -m 'cherry work'; git switch master; \
                      git cherry-pick cherry-source; git revert --no-edit HEAD";
        let (code, output) = shell_git(&mut shell, script).await;
        if code != 0 {
            eprintln!("FAIL YourShell scenario exited {code}\n{output}");
            return false;
        }

        let expected = repository_snapshot(&oracle);
        let got = repository_snapshot(&actual);
        if expected != got {
            eprintln!(
                "FAIL repository state differs\n--- desktop ---\n{expected}\n--- YourShell ---\n{got}\n--- command output ---\n{output}"
            );
            return false;
        }
        println!("PASS local repository state");

        std::fs::write(oracle.join("one.txt"), "stashed change\n").expect("oracle stash change");
        std::fs::write(oracle.join("untracked.txt"), "untracked\n").expect("oracle untracked");
        let (code, output) =
            desktop_git(&oracle, &["stash", "push", "-u", "-m", "saved state"]);
        if code != 0 {
            eprintln!("FAIL desktop stash push: {output}");
            return false;
        }
        if !desktop_git(&oracle, &["status", "--porcelain"]).1.is_empty()
            || !desktop_git(&oracle, &["stash", "list"])
                .1
                .contains("saved state")
        {
            eprintln!("FAIL desktop stash precondition");
            return false;
        }
        assert_eq!(desktop_git(&oracle, &["stash", "pop"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            "printf 'stashed change\\n' > one.txt; printf 'untracked\\n' > untracked.txt; \
             git stash push -u -m 'saved state'; \
             test -z \"$(git status --porcelain)\" && git stash list | grep 'saved state' >/dev/null && \
             git stash pop",
        )
        .await;
        if code != 0 {
            eprintln!("FAIL YourShell stash workflow exited {code}\n{output}");
            return false;
        }
        let expected = repository_snapshot(&oracle);
        let got = repository_snapshot(&actual);
        if expected != got
            || std::fs::read(oracle.join("one.txt")).ok()
                != std::fs::read(actual.join("one.txt")).ok()
            || std::fs::read(oracle.join("untracked.txt")).ok()
                != std::fs::read(actual.join("untracked.txt")).ok()
        {
            eprintln!(
                "FAIL stash push/list/pop differs\n--- desktop ---\n{expected}\n--- YourShell ---\n{got}\n{output}"
            );
            return false;
        }
        println!("PASS stash push/list/pop");

        let oracle_clone = base.join("oracle-clone");
        let actual_clone = base.join("actual-clone");
        let oracle_text = oracle.to_string_lossy().into_owned();
        let oracle_clone_text = oracle_clone.to_string_lossy().into_owned();
        let clone_args = [
            "clone",
            "-q",
            "--branch",
            "feature",
            "--origin",
            "upstream",
            oracle_text.as_str(),
            oracle_clone_text.as_str(),
        ];
        let (code, output) = desktop_git(&base, &clone_args);
        if code != 0 {
            eprintln!("FAIL desktop clone: {output}");
            return false;
        }
        let actual_text = actual.to_string_lossy();
        let actual_clone_text = actual_clone.to_string_lossy();
        let clone_script = format!(
            "git clone -q --branch feature --origin upstream {actual_text} {actual_clone_text}"
        );
        let (code, output) = shell_git(&mut shell, &clone_script).await;
        if code != 0 {
            eprintln!("FAIL YourShell clone exited {code}\n{output}");
            return false;
        }
        let expected = repository_snapshot(&oracle_clone);
        let got = repository_snapshot(&actual_clone);
        let expected_remote = desktop_git(&oracle_clone, &["remote"]).1;
        let got_remote = desktop_git(&actual_clone, &["remote"]).1;
        if expected != got || expected_remote != got_remote {
            eprintln!(
                "FAIL cloned repository differs\n--- desktop ---\n{expected}\nremote={expected_remote}\n--- YourShell ---\n{got}\nremote={got_remote}"
            );
            return false;
        }
        println!("PASS clone branch/origin repository state");

        let oracle_conflict = base.join("oracle-conflict");
        let actual_conflict = base.join("actual-conflict");
        std::fs::create_dir(&oracle_conflict).expect("oracle conflict dir");
        std::fs::create_dir(&actual_conflict).expect("actual conflict dir");
        desktop_setup(&oracle_conflict);
        std::fs::write(oracle_conflict.join("conflict.txt"), "base\n").expect("base");
        for args in [
            &["add", "conflict.txt"][..],
            &["commit", "-q", "-m", "base"][..],
            &["switch", "-q", "-c", "topic"][..],
        ] {
            assert_eq!(desktop_git(&oracle_conflict, args).0, 0);
        }
        std::fs::write(oracle_conflict.join("conflict.txt"), "topic\n").expect("topic");
        for args in [
            &["commit", "-q", "-am", "topic"][..],
            &["switch", "-q", "master"][..],
        ] {
            assert_eq!(desktop_git(&oracle_conflict, args).0, 0);
        }
        std::fs::write(oracle_conflict.join("conflict.txt"), "master\n").expect("master");
        assert_eq!(
            desktop_git(&oracle_conflict, &["commit", "-q", "-am", "master"]).0,
            0
        );
        assert_ne!(desktop_git(&oracle_conflict, &["merge", "topic"]).0, 0);
        assert!(
            desktop_git(&oracle_conflict, &["status", "--porcelain"])
                .1
                .contains("UU conflict.txt")
        );
        assert_eq!(
            desktop_git(&oracle_conflict, &["merge", "--abort"]).0,
            0
        );

        let actual_conflict_text = actual_conflict.to_string_lossy();
        let conflict_script = format!(
            "cd {actual_conflict_text}; git init -q; \
             git config user.name 'YourShell Test'; git config user.email test@yourshell.local; \
             printf 'base\\n' > conflict.txt; git add conflict.txt; git commit -m base; \
             git switch -c topic; printf 'topic\\n' > conflict.txt; git add conflict.txt; \
             git commit -m topic; git switch master; printf 'master\\n' > conflict.txt; \
             git add conflict.txt; git commit -m master; git merge topic; \
             test $? -ne 0 && git status --porcelain | grep '^UU conflict.txt' >/dev/null && \
             git merge --abort"
        );
        let (code, output) = shell_git(&mut shell, &conflict_script).await;
        if code != 0 {
            eprintln!("FAIL conflict/abort scenario exited {code}\n{output}");
            return false;
        }
        let expected = repository_snapshot(&oracle_conflict);
        let got = repository_snapshot(&actual_conflict);
        if expected != got
            || std::fs::read(&oracle_conflict.join("conflict.txt")).ok()
                != std::fs::read(&actual_conflict.join("conflict.txt")).ok()
        {
            eprintln!(
                "FAIL merge abort state differs\n--- desktop ---\n{expected}\n--- YourShell ---\n{got}\n{output}"
            );
            return false;
        }
        println!("PASS merge conflict detection and abort");

        for command in ["cherry-pick", "revert"] {
            for should_continue in [true, false] {
                if !replay_conflict_case(&mut shell, &base, command, should_continue).await {
                    return false;
                }
            }
        }
        for should_continue in [true, false] {
            if !rebase_conflict_case(&mut shell, &base, should_continue).await {
                return false;
            }
        }

        // restore: worktree from index, then index + worktree from HEAD.
        std::fs::write(oracle.join("one.txt"), "worktree edit\n").expect("oracle restore edit");
        std::fs::write(actual.join("one.txt"), "worktree edit\n").expect("actual restore edit");
        assert_eq!(desktop_git(&oracle, &["restore", "one.txt"]).0, 0);
        let actual_text = actual.to_string_lossy();
        let (code, output) =
            shell_git(&mut shell, &format!("git -C {actual_text} restore one.txt")).await;
        if code != 0 {
            eprintln!("FAIL restore worktree exited {code}\n{output}");
            return false;
        }
        std::fs::write(oracle.join("one.txt"), "staged edit\n").expect("oracle staged edit");
        std::fs::write(actual.join("one.txt"), "staged edit\n").expect("actual staged edit");
        assert_eq!(desktop_git(&oracle, &["add", "one.txt"]).0, 0);
        let (code, output) =
            shell_git(&mut shell, &format!("git -C {actual_text} add one.txt")).await;
        if code != 0 {
            eprintln!("FAIL stage before restore exited {code}\n{output}");
            return false;
        }
        assert_eq!(
            desktop_git(&oracle, &["restore", "--staged", "--worktree", "one.txt"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} restore --staged --worktree one.txt"),
        )
        .await;
        if code != 0
            || repository_snapshot(&oracle) != repository_snapshot(&actual)
            || std::fs::read(oracle.join("one.txt")).ok()
                != std::fs::read(actual.join("one.txt")).ok()
        {
            eprintln!("FAIL restore staged/worktree differs [{code}]\n{output}");
            return false;
        }
        println!("PASS restore worktree and staged state");

        // clean must preserve its force safety boundary and directory policy.
        for repo in [&oracle, &actual] {
            std::fs::write(repo.join("clean-file.tmp"), "remove\n").expect("clean file");
            std::fs::create_dir(repo.join("clean-dir")).expect("clean dir");
            std::fs::write(repo.join("clean-dir/item"), "remove\n").expect("clean dir file");
        }
        assert_eq!(desktop_git(&oracle, &["clean", "-fd"]).0, 0);
        let (code, output) =
            shell_git(&mut shell, &format!("git -C {actual_text} clean -fd")).await;
        if code != 0
            || oracle.join("clean-file.tmp").exists() != actual.join("clean-file.tmp").exists()
            || oracle.join("clean-dir").exists() != actual.join("clean-dir").exists()
        {
            eprintln!("FAIL clean -fd differs [{code}]\n{output}");
            return false;
        }
        println!("PASS clean force/directory behavior");

        // `commit -am` is the most common tracked-file shortcut.
        std::fs::write(oracle.join("one.txt"), "commit all\n").expect("oracle commit -am");
        std::fs::write(actual.join("one.txt"), "commit all\n").expect("actual commit -am");
        assert_eq!(
            desktop_git(&oracle, &["commit", "-q", "-am", "commit all"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} commit -am 'commit all'"),
        )
        .await;
        if code != 0 || repository_snapshot(&oracle) != repository_snapshot(&actual) {
            eprintln!("FAIL commit -am differs [{code}]\n{output}");
            return false;
        }
        println!("PASS commit -am");

        // Common diff views must match Git byte-for-byte.
        for repo in [&oracle, &actual] {
            std::fs::write(repo.join("staged-diff.txt"), "staged\n").expect("staged diff file");
        }
        assert_eq!(desktop_git(&oracle, &["add", "staged-diff.txt"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} add staged-diff.txt"),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL stage diff fixture [{code}]\n{output}");
            return false;
        }
        for args in [
            &["diff", "--staged"][..],
            &["diff", "--staged", "--name-only"][..],
            &["diff", "--staged", "--stat"][..],
        ] {
            let (expected_code, expected_output) = desktop_git(&oracle, args);
            let (actual_code, actual_output) = shell_git(
                &mut shell,
                &format!("git -C {actual_text} {}", args.join(" ")),
            )
            .await;
            if actual_code != expected_code || actual_output != expected_output {
                eprintln!(
                    "FAIL diff `{}` differs\n--- desktop [{expected_code}] ---\n{expected_output}\
                     --- YourShell [{actual_code}] ---\n{actual_output}",
                    args.join(" ")
                );
                return false;
            }
        }
        assert_eq!(desktop_git(&oracle, &["reset", "--hard", "HEAD"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} reset --hard HEAD"),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL reset after diff fixture [{code}]\n{output}");
            return false;
        }
        println!("PASS diff staged/name-only/stat");

        // Amend and allow-empty affect history rather than just output.
        assert_eq!(
            desktop_git(&oracle, &["commit", "-q", "--amend", "-m", "amended common"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} commit --amend -m 'amended common'"),
        )
        .await;
        if code != 0
            || desktop_git(&oracle, &["log", "-1", "--format=%s"]).1
                != desktop_git(&actual, &["log", "-1", "--format=%s"]).1
            || repository_snapshot(&oracle) != repository_snapshot(&actual)
        {
            eprintln!("FAIL commit --amend differs [{code}]\n{output}");
            return false;
        }
        assert_eq!(
            desktop_git(&oracle, &["commit", "-q", "--allow-empty", "-m", "empty common"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} commit --allow-empty -m 'empty common'"),
        )
        .await;
        if code != 0
            || desktop_git(&oracle, &["log", "-1", "--format=%s"]).1
                != desktop_git(&actual, &["log", "-1", "--format=%s"]).1
        {
            eprintln!("FAIL commit --allow-empty differs [{code}]\n{output}");
            return false;
        }
        println!("PASS commit amend/allow-empty");

        // Common branch maintenance: create from an explicit start point,
        // rename, then safely delete a merged branch.
        for args in [
            &["branch", "maintenance", "master"][..],
            &["branch", "-m", "maintenance", "maint-renamed"][..],
            &["branch", "-d", "maint-renamed"][..],
        ] {
            if desktop_git(&oracle, args).0 != 0 {
                eprintln!("FAIL desktop branch maintenance: {}", args.join(" "));
                return false;
            }
            let (code, output) = shell_git(
                &mut shell,
                &format!("git -C {actual_text} {}", args.join(" ")),
            )
            .await;
            if code != 0 {
                eprintln!("FAIL branch maintenance `{}` [{code}]\n{output}", args.join(" "));
                return false;
            }
        }
        if repository_snapshot(&oracle) != repository_snapshot(&actual) {
            eprintln!("FAIL branch create/rename/delete state differs");
            return false;
        }
        println!("PASS branch create/rename/delete");

        // Common remote management and its script-consumed output.
        for args in [
            &["remote", "add", "demo", "https://example.com/one.git"][..],
            &["remote", "set-url", "demo", "https://example.com/two.git"][..],
            &["remote", "rename", "demo", "upstream-demo"][..],
        ] {
            if desktop_git(&oracle, args).0 != 0 {
                eprintln!("FAIL desktop remote maintenance: {}", args.join(" "));
                return false;
            }
            let (code, output) = shell_git(
                &mut shell,
                &format!("git -C {actual_text} {}", args.join(" ")),
            )
            .await;
            if code != 0 {
                eprintln!("FAIL remote maintenance `{}` [{code}]\n{output}", args.join(" "));
                return false;
            }
        }
        for args in [
            &["remote", "-v"][..],
            &["remote", "get-url", "upstream-demo"][..],
            &["log", "--oneline", "-2"][..],
        ] {
            let expected_repo = if args.first() == Some(&"log") {
                &actual
            } else {
                &oracle
            };
            let (expected_code, expected_output) = desktop_git(expected_repo, args);
            let (actual_code, actual_output) = shell_git(
                &mut shell,
                &format!("git -C {actual_text} {}", args.join(" ")),
            )
            .await;
            if actual_code != expected_code || actual_output != expected_output {
                eprintln!(
                    "FAIL common output `{}` differs\n--- desktop [{expected_code}] ---\n{expected_output}\
                     --- YourShell [{actual_code}] ---\n{actual_output}",
                    args.join(" ")
                );
                return false;
            }
        }
        assert_eq!(
            desktop_git(&oracle, &["remote", "remove", "upstream-demo"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("git -C {actual_text} remote remove upstream-demo"),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL remote remove [{code}]\n{output}");
            return false;
        }
        println!("PASS remote management and oneline log");

        for args in [
            &["config", "test.compat", "value"][..],
            &["config", "--get", "test.compat"][..],
            &["config", "--unset", "test.compat"][..],
        ] {
            let (expected_code, expected_output) = desktop_git(&oracle, args);
            let (actual_code, actual_output) = shell_git(
                &mut shell,
                &format!("git -C {actual_text} {}", args.join(" ")),
            )
            .await;
            if actual_code != expected_code || actual_output != expected_output {
                eprintln!(
                    "FAIL config `{}` differs\n--- desktop [{expected_code}] ---\n{expected_output}\
                     --- YourShell [{actual_code}] ---\n{actual_output}",
                    args.join(" ")
                );
                return false;
            }
        }
        println!("PASS config set/get/unset");

        // Real local transport: upstream configuration, ordinary push, and a
        // non-fast-forward rewrite guarded by force-with-lease.
        let oracle_push = base.join("oracle-push-source");
        let actual_push = base.join("actual-push-source");
        let oracle_bare = base.join("oracle-push.git");
        let actual_bare = base.join("actual-push.git");
        std::fs::create_dir(&oracle_push).expect("oracle push source");
        std::fs::create_dir(&actual_push).expect("actual push source");
        desktop_setup(&oracle_push);
        std::fs::write(oracle_push.join("push.txt"), "one\n").expect("oracle push file");
        for args in [
            &["add", "push.txt"][..],
            &["commit", "-q", "-m", "push one"][..],
        ] {
            assert_eq!(desktop_git(&oracle_push, args).0, 0);
        }
        assert_eq!(
            desktop_git(
                &base,
                &["init", "-q", "--bare", oracle_bare.to_string_lossy().as_ref()]
            )
            .0,
            0
        );
        let actual_push_text = actual_push.to_string_lossy();
        let actual_bare_text = actual_bare.to_string_lossy();
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git init -q; git config user.name 'YourShell Test'; \
                 git config user.email test@yourshell.local; printf 'one\\n' > push.txt; \
                 git add push.txt; git commit -m 'push one'; git init -q --bare {actual_bare_text}"
            ),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL push fixture setup [{code}]\n{output}");
            return false;
        }
        let oracle_bare_text = oracle_bare.to_string_lossy().into_owned();
        assert_eq!(
            desktop_git(
                &oracle_push,
                &["remote", "add", "origin", &oracle_bare_text]
            )
            .0,
            0
        );
        assert_eq!(
            desktop_git(&oracle_push, &["push", "-q", "-u", "origin", "master"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git remote add origin {actual_bare_text}; \
                 git push -u origin master"
            ),
        )
        .await;
        if code != 0
            || desktop_git(
                &actual_push,
                &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
            )
            .1
            .trim()
                != "origin/master"
        {
            eprintln!("FAIL push -u/upstream [{code}]\n{output}");
            return false;
        }
        for repo in [&oracle_push, &actual_push] {
            std::fs::write(repo.join("push.txt"), "two\n").expect("push second");
        }
        assert_eq!(desktop_git(&oracle_push, &["commit", "-q", "-am", "push two"]).0, 0);
        assert_eq!(desktop_git(&oracle_push, &["push", "-q"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git commit -am 'push two'; git push"),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL ordinary tracked push [{code}]\n{output}");
            return false;
        }
        assert_eq!(desktop_git(&oracle_push, &["reset", "--hard", "HEAD~1"]).0, 0);
        std::fs::write(oracle_push.join("push.txt"), "rewritten\n").expect("oracle rewrite");
        assert_eq!(
            desktop_git(&oracle_push, &["commit", "-q", "-am", "rewritten"]).0,
            0
        );
        assert_eq!(
            desktop_git(&oracle_push, &["push", "-q", "--force-with-lease"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git reset --hard HEAD~1; \
                 printf 'rewritten\\n' > push.txt; git commit -am rewritten; \
                 git push --force-with-lease"
            ),
        )
        .await;
        let expected_tree = desktop_git(&oracle_bare, &["rev-parse", "master^{tree}"]).1;
        let actual_tree = desktop_git(&actual_bare, &["rev-parse", "master^{tree}"]).1;
        if code != 0 || expected_tree != actual_tree {
            eprintln!(
                "FAIL force-with-lease [{code}]\n{output}\nexpected tree {expected_tree}actual tree {actual_tree}"
            );
            return false;
        }
        println!("PASS push upstream and force-with-lease");

        // Pull must fast-forward when requested and create a merge commit for
        // an ordinary divergent pull instead of returning a false success.
        let oracle_writer = base.join("oracle-pull-writer");
        let actual_writer = base.join("actual-pull-writer");
        assert_eq!(
            desktop_git(
                &base,
                &[
                    "clone",
                    "-q",
                    &oracle_bare_text,
                    oracle_writer.to_string_lossy().as_ref(),
                ],
            )
            .0,
            0
        );
        assert_eq!(
            desktop_git(
                &base,
                &[
                    "clone",
                    "-q",
                    actual_bare_text.as_ref(),
                    actual_writer.to_string_lossy().as_ref(),
                ],
            )
            .0,
            0
        );
        for writer in [&oracle_writer, &actual_writer] {
            assert_eq!(
                desktop_git(writer, &["config", "user.name", "Remote Writer"]).0,
                0
            );
            assert_eq!(
                desktop_git(writer, &["config", "user.email", "remote@yourshell.local"]).0,
                0
            );
            std::fs::write(writer.join("remote.txt"), "remote one\n").expect("remote one");
            assert_eq!(desktop_git(writer, &["add", "remote.txt"]).0, 0);
            assert_eq!(
                desktop_git(writer, &["commit", "-q", "-m", "remote one"]).0,
                0
            );
            assert_eq!(desktop_git(writer, &["push", "-q"]).0, 0);
        }
        assert_eq!(desktop_git(&oracle_push, &["pull", "-q", "--ff-only"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git pull --ff-only"),
        )
        .await;
        if code != 0
            || std::fs::read(oracle_push.join("remote.txt")).ok()
                != std::fs::read(actual_push.join("remote.txt")).ok()
        {
            eprintln!("FAIL pull --ff-only [{code}]\n{output}");
            return false;
        }

        for source in [&oracle_push, &actual_push] {
            std::fs::write(source.join("local.txt"), "local divergence\n")
                .expect("local divergence");
        }
        assert_eq!(desktop_git(&oracle_push, &["add", "local.txt"]).0, 0);
        assert_eq!(
            desktop_git(&oracle_push, &["commit", "-q", "-m", "local divergence"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git add local.txt; git commit -m 'local divergence'"
            ),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL local divergence setup [{code}]\n{output}");
            return false;
        }
        for writer in [&oracle_writer, &actual_writer] {
            std::fs::write(writer.join("remote-two.txt"), "remote two\n").expect("remote two");
            assert_eq!(desktop_git(writer, &["add", "remote-two.txt"]).0, 0);
            assert_eq!(
                desktop_git(writer, &["commit", "-q", "-m", "remote two"]).0,
                0
            );
            assert_eq!(desktop_git(writer, &["push", "-q"]).0, 0);
        }
        if desktop_git(&oracle_push, &["pull", "-q", "--ff-only"]).0 == 0 {
            eprintln!("FAIL desktop ff-only divergence precondition");
            return false;
        }
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git pull --ff-only"),
        )
        .await;
        if code == 0 {
            eprintln!("FAIL pull --ff-only accepted divergence\n{output}");
            return false;
        }
        assert_eq!(
            desktop_git(&oracle_push, &["pull", "-q", "--no-rebase"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git pull --no-rebase"),
        )
        .await;
        let expected_tree = desktop_git(&oracle_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_tree = desktop_git(&actual_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_parents =
            desktop_git(&actual_push, &["show", "-s", "--format=%P", "HEAD"]).1;
        if code != 0
            || expected_tree != actual_tree
            || actual_parents.split_whitespace().count() != 2
        {
            eprintln!(
                "FAIL divergent pull merge [{code}]\n{output}\nexpected tree {expected_tree}actual tree {actual_tree}parents {actual_parents}"
            );
            return false;
        }
        println!("PASS pull fast-forward and divergent merge");

        // A lease must reject when another writer advanced the remote after
        // our last observation.
        for writer in [&oracle_writer, &actual_writer] {
            std::fs::write(writer.join("remote-three.txt"), "remote three\n")
                .expect("remote three");
            assert_eq!(desktop_git(writer, &["add", "remote-three.txt"]).0, 0);
            assert_eq!(
                desktop_git(writer, &["commit", "-q", "-m", "remote three"]).0,
                0
            );
            assert_eq!(desktop_git(writer, &["push", "-q"]).0, 0);
        }
        if desktop_git(&oracle_push, &["push", "--force-with-lease"]).0 == 0 {
            eprintln!("FAIL desktop stale-lease precondition");
            return false;
        }
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git push --force-with-lease"),
        )
        .await;
        if code == 0 || !output.contains("stale info") {
            eprintln!("FAIL stale force-with-lease was not rejected [{code}]\n{output}");
            return false;
        }
        println!("PASS ff-only and stale-lease rejection");

        // The common pull --rebase path: replay the local divergent commit on
        // top of the newly advanced remote and leave a linear history.
        assert_eq!(
            desktop_git(&oracle_push, &["pull", "-q", "--rebase"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git pull --rebase"),
        )
        .await;
        let expected_tree = desktop_git(&oracle_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_tree = desktop_git(&actual_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_parents =
            desktop_git(&actual_push, &["show", "-s", "--format=%P", "HEAD"]).1;
        if code != 0
            || expected_tree != actual_tree
            || actual_parents.split_whitespace().count() != 1
        {
            eprintln!(
                "FAIL pull --rebase [{code}]\n{output}\nexpected tree {expected_tree}actual tree {actual_tree}parents {actual_parents}"
            );
            return false;
        }
        println!("PASS pull --rebase linear history");

        // pull --rebase conflict lifecycle: the follow-up is the ordinary
        // `git rebase --continue` / `--abort` workflow.
        for source in [&oracle_push, &actual_push] {
            std::fs::write(source.join("push.txt"), "local rebase conflict\n")
                .expect("local rebase conflict");
        }
        assert_eq!(
            desktop_git(&oracle_push, &["commit", "-q", "-am", "local rebase conflict"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git commit -am 'local rebase conflict'"
            ),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL local pull-rebase conflict setup [{code}]\n{output}");
            return false;
        }
        for writer in [&oracle_writer, &actual_writer] {
            std::fs::write(writer.join("push.txt"), "remote rebase conflict\n")
                .expect("remote rebase conflict");
            assert_eq!(
                desktop_git(writer, &["commit", "-q", "-am", "remote rebase conflict"]).0,
                0
            );
            assert_eq!(desktop_git(writer, &["push", "-q"]).0, 0);
        }
        if desktop_git(&oracle_push, &["pull", "-q", "--rebase"]).0 == 0 {
            eprintln!("FAIL desktop pull-rebase conflict precondition");
            return false;
        }
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git pull --rebase"),
        )
        .await;
        if code == 0
            || !desktop_git(&actual_push, &["status", "--porcelain"])
                .1
                .contains("UU push.txt")
        {
            eprintln!("FAIL pull --rebase conflict not exposed [{code}]\n{output}");
            return false;
        }
        for source in [&oracle_push, &actual_push] {
            std::fs::write(source.join("push.txt"), "resolved rebase conflict\n")
                .expect("resolve pull rebase");
        }
        assert_eq!(desktop_git(&oracle_push, &["add", "push.txt"]).0, 0);
        assert_eq!(desktop_git(&oracle_push, &["rebase", "--continue"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git add push.txt; git rebase --continue"
            ),
        )
        .await;
        let expected_tree = desktop_git(&oracle_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_tree = desktop_git(&actual_push, &["rev-parse", "HEAD^{tree}"]).1;
        if code != 0 || expected_tree != actual_tree {
            eprintln!(
                "FAIL pull-rebase continue [{code}]\n{output}\nexpected {expected_tree}actual {actual_tree}"
            );
            return false;
        }
        println!("PASS pull --rebase conflict continue");

        // Build one more conflict and prove abort restores the exact pre-pull
        // local tree and clears repository state.
        for source in [&oracle_push, &actual_push] {
            std::fs::write(source.join("push.txt"), "local abort value\n")
                .expect("local abort value");
        }
        assert_eq!(
            desktop_git(&oracle_push, &["commit", "-q", "-am", "local abort value"]).0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git commit -am 'local abort value'"),
        )
        .await;
        if code != 0 {
            eprintln!("FAIL rebase abort local setup [{code}]\n{output}");
            return false;
        }
        let oracle_before_abort = desktop_git(&oracle_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_before_abort = desktop_git(&actual_push, &["rev-parse", "HEAD^{tree}"]).1;
        for writer in [&oracle_writer, &actual_writer] {
            std::fs::write(writer.join("push.txt"), "remote abort value\n")
                .expect("remote abort value");
            assert_eq!(
                desktop_git(writer, &["commit", "-q", "-am", "remote abort value"]).0,
                0
            );
            assert_eq!(desktop_git(writer, &["push", "-q"]).0, 0);
        }
        assert_ne!(
            desktop_git(&oracle_push, &["pull", "-q", "--rebase"]).0,
            0
        );
        assert_eq!(desktop_git(&oracle_push, &["rebase", "--abort"]).0, 0);
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; git pull --rebase; \
                 test $? -ne 0 && git rebase --abort"
            ),
        )
        .await;
        let oracle_after_abort = desktop_git(&oracle_push, &["rev-parse", "HEAD^{tree}"]).1;
        let actual_after_abort = desktop_git(&actual_push, &["rev-parse", "HEAD^{tree}"]).1;
        if code != 0
            || oracle_before_abort != oracle_after_abort
            || actual_before_abort != actual_after_abort
            || desktop_git(&actual_push, &["status", "--porcelain"]).1 != ""
        {
            eprintln!(
                "FAIL pull-rebase abort [{code}]\n{output}\nbefore {actual_before_abort}after {actual_after_abort}"
            );
            return false;
        }
        println!("PASS pull --rebase conflict abort");

        // Autostash must survive a pull-rebase conflict and be reapplied only
        // after the rebase is successfully continued.
        for source in [&oracle_push, &actual_push] {
            std::fs::write(source.join("local.txt"), "dirty autostash retained\n")
                .expect("autostash dirty file");
        }
        assert_ne!(
            desktop_git(
                &oracle_push,
                &["pull", "-q", "--rebase", "--autostash"]
            )
            .0,
            0
        );
        let (code, output) = shell_git(
            &mut shell,
            &format!("cd {actual_push_text}; git pull --rebase --autostash"),
        )
        .await;
        if code == 0
            || !desktop_git(&actual_push, &["status", "--porcelain"])
                .1
                .contains("UU push.txt")
        {
            eprintln!("FAIL pull-rebase autostash conflict [{code}]\n{output}");
            return false;
        }
        for _ in 0..4 {
            if !rebase_state_exists(&oracle_push) {
                break;
            }
            std::fs::write(
                oracle_push.join("push.txt"),
                "resolved abort-cycle conflict\n",
            )
            .expect("resolve oracle autostash rebase");
            assert_eq!(desktop_git(&oracle_push, &["add", "push.txt"]).0, 0);
            let _ = desktop_git(&oracle_push, &["rebase", "--continue"]);
        }
        if rebase_state_exists(&oracle_push) {
            eprintln!("FAIL desktop autostash rebase did not finish");
            return false;
        }
        let (code, output) = shell_git(
            &mut shell,
            &format!(
                "cd {actual_push_text}; \
                 for i in 1 2 3 4; do \
                   test ! -d .git/rebase-merge -a ! -d .git/rebase-apply && break; \
                   printf 'resolved abort-cycle conflict\\n' > push.txt; \
                   git add push.txt; git rebase --continue || true; \
                 done; \
                 test ! -d .git/rebase-merge -a ! -d .git/rebase-apply"
            ),
        )
        .await;
        if code != 0
            || std::fs::read_to_string(actual_push.join("local.txt")).ok().as_deref()
                != Some("dirty autostash retained\n")
            || !desktop_git(&actual_push, &["status", "--porcelain"])
                .1
                .contains(" M local.txt")
            || rebase_state_exists(&actual_push)
        {
            eprintln!("FAIL autostash restore after continue [{code}]\n{output}");
            return false;
        }
        println!("PASS pull --rebase autostash conflict lifecycle");

        // The normal configured workflow (`pull.rebase=true` and
        // `rebase.autostash=true`) must behave the same without CLI flags.
        for source in [&oracle_push, &actual_push] {
            assert_eq!(
                desktop_git(source, &["config", "pull.rebase", "true"]).0,
                0
            );
            assert_eq!(
                desktop_git(source, &["config", "rebase.autostash", "true"]).0,
                0
            );
        }
        for writer in [&oracle_writer, &actual_writer] {
            std::fs::write(writer.join("configured-pull.txt"), "configured pull\n")
                .expect("configured pull");
            assert_eq!(desktop_git(writer, &["add", "configured-pull.txt"]).0, 0);
            assert_eq!(
                desktop_git(writer, &["commit", "-q", "-m", "configured pull"]).0,
                0
            );
            assert_eq!(desktop_git(writer, &["push", "-q"]).0, 0);
        }
        assert_eq!(desktop_git(&oracle_push, &["pull", "-q"]).0, 0);
        let (code, output) =
            shell_git(&mut shell, &format!("cd {actual_push_text}; git pull")).await;
        if code != 0
            || std::fs::read_to_string(actual_push.join("local.txt")).ok().as_deref()
                != Some("dirty autostash retained\n")
            || std::fs::read(oracle_push.join("configured-pull.txt")).ok()
                != std::fs::read(actual_push.join("configured-pull.txt")).ok()
        {
            eprintln!("FAIL configured pull.rebase/autostash [{code}]\n{output}");
            return false;
        }
        println!("PASS configured pull.rebase and rebase.autostash");

        // Script-facing plumbing must preserve stdout because shell scripts
        // consume it directly. Compare the same repository through desktop
        // Git and the in-process adapter.
        for args in [
            &["rev-parse", "HEAD"][..],
            &["rev-parse", "--show-toplevel"][..],
            &["rev-parse", "--is-bare-repository"][..],
            &["ls-files"][..],
            &["ls-files", "--stage"][..],
            &["ls-tree", "--name-only", "HEAD"][..],
            &["ls-tree", "-r", "--name-only", "HEAD"][..],
            &["show-ref", "--heads"][..],
            &["show-ref", "--tags"][..],
            &["symbolic-ref", "HEAD"][..],
            &["merge-base", "master", "feature"][..],
        ] {
            let (expected_code, expected_output) = desktop_git(&actual, args);
            let script = format!("git -C {} {}", actual.to_string_lossy(), args.join(" "));
            let (actual_code, actual_output) = shell_git(&mut shell, &script).await;
            if actual_code != expected_code || actual_output != expected_output {
                eprintln!(
                    "FAIL plumbing `{script}` differs\n--- desktop [{expected_code}] ---\n{expected_output}\
                     --- YourShell [{actual_code}] ---\n{actual_output}"
                );
                return false;
            }
        }
        println!("PASS plumbing stdout and exit status");

        if std::env::var_os("YOURSHELL_GIT_NETWORK_TEST").is_some() {
            let (code, output) = shell_git(
                &mut shell,
                "git ls-remote --heads https://github.com/octocat/Hello-World.git master",
            )
            .await;
            if code != 0 || !output.contains("\trefs/heads/master") {
                eprintln!("FAIL HTTPS ls-remote exited {code}\n{output}");
                return false;
            }
            println!("PASS HTTPS ls-remote");

            let network_clone = base.join("https-clone");
            let network_clone_text = network_clone.to_string_lossy();
            let script = format!(
                "git clone -q --depth 1 --branch master https://github.com/octocat/Hello-World.git {network_clone_text}"
            );
            let (code, output) = shell_git(&mut shell, &script).await;
            if code != 0 {
                eprintln!("FAIL HTTPS shallow clone exited {code}\n{output}");
                return false;
            }
            let (code, head) = desktop_git(&network_clone, &["rev-parse", "--verify", "HEAD"]);
            if code != 0 || head.trim().len() != 40 {
                eprintln!("FAIL HTTPS clone repository invalid: {head}");
                return false;
            }
            println!("PASS HTTPS shallow branch clone");
        }
        true
    });
    if !ok {
        std::process::exit(1);
    }
}
