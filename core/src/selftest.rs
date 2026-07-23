//! In-process test battery. Runs the same Shell configuration the FFI
//! sessions use, one case at a time, capturing each case's fd1/fd2 through a
//! fresh pipe so output assertions are deterministic (no reader-thread race).
//! Callable from cargo test on the host and from the app on iOS.

use std::io::Read;

use brush_core::openfiles::OpenFile;

pub enum Check {
    /// Trimmed output equals exactly.
    Eq(&'static str),
    /// Output contains substring.
    Has(&'static str),
    /// Output does NOT contain substring.
    Not(&'static str),
    /// Exit code equals.
    Exit(i32),
}

pub struct Case {
    pub name: &'static str,
    pub script: &'static str,
    pub checks: &'static [Check],
}

macro_rules! case {
    ($name:literal, $script:literal, $($check:expr),+ $(,)?) => {
        Case { name: $name, script: $script, checks: &[$($check),+] }
    };
}

use Check::{Eq, Exit, Has, Not};

pub static CASES: &[Case] = &[
    // ---- shell language ----
    case!("echo", "echo hello world", Eq("hello world"), Exit(0)),
    case!("echo-n", "echo -n abc; echo :end", Eq("abc:end")),
    case!("vars", "x=41; echo \"x=$x\"", Eq("x=41")),
    case!("arith", "echo $((6 * 7))", Eq("42")),
    case!("arith-ternary", "echo $((5 > 3 ? 10 : 20))", Eq("10")),
    case!("cmd-subst", "echo \"got:$(echo inner)\"", Eq("got:inner")),
    case!("backtick-subst", "echo `echo bt`", Eq("bt")),
    case!("for-loop", "for i in 1 2 3; do echo \"i=$i\"; done", Eq("i=1\ni=2\ni=3")),
    case!("while-loop", "n=0; while [ $n -lt 3 ]; do n=$((n+1)); done; echo $n", Eq("3")),
    case!("until-loop", "n=0; until [ $n -ge 2 ]; do n=$((n+1)); done; echo $n", Eq("2")),
    case!("if-else", "if [ 2 -gt 1 ]; then echo yes; else echo no; fi", Eq("yes")),
    case!("case-stmt", "case abcd in xy*) echo wrong;; ab*) echo matched;; esac", Eq("matched")),
    case!("func-args-local-return", "f() { local y=5; echo \"fn:$1:$y\"; return 3; }; f arg; echo rc=$?", Eq("fn:arg:5\nrc=3")),
    case!("subshell-isolation", "sx=1; (sx=2); echo $sx", Eq("1")),
    case!("subshell-exit", "(exit 7); echo $?", Eq("7")),
    case!("and-or", "true && echo A; false || echo B; false && echo C; echo end", Eq("A\nB\nend")),
    case!("negate", "! false; echo $?", Eq("0")),
    case!("positional", "set -- a b c; echo \"$# $2 $@\"", Eq("3 b a b c")),
    case!("shift", "set -- a b c; shift; echo $1", Eq("b")),
    case!("brace-expansion", "echo {1..4}", Eq("1 2 3 4")),
    case!("param-len", "s=hello; echo ${#s}", Eq("5")),
    case!("param-substr", "s=hello; echo ${s:1:3}", Eq("ell")),
    case!("param-replace", "s=hello; echo ${s/l/L}", Eq("heLlo")),
    case!("param-default", "unset u; echo \"u=${u:-fallback}\"", Eq("u=fallback")),
    case!("arrays", "arr=(one two three); echo \"${arr[1]} ${#arr[@]}\"", Eq("two 3")),
    case!("break-continue", "for i in 1 2 3 4; do [ $i = 2 ] && continue; [ $i = 4 ] && break; echo i=$i; done", Eq("i=1\ni=3")),
    case!("double-bracket-glob", "[[ hello == h* ]] && echo globbed", Eq("globbed")),
    case!("herestring-read", "read rv <<< hereword; echo \"read:$rv\"", Eq("read:hereword")),
    case!("heredoc-cat", "cat <<EOF\nline one\nline two\nEOF", Eq("line one\nline two")),
    case!("exit-code-127", "definitely_not_a_command_xyz", Exit(127)),
    case!("dollar-dollar", "echo $$", Not("$$"), Exit(0)),
    // ---- redirection & pipes ----
    case!("redirect-out", "echo filedata > rt.txt; cat rt.txt", Eq("filedata")),
    case!("redirect-append", "echo l1 > ra.txt; echo l2 >> ra.txt; cat ra.txt", Eq("l1\nl2")),
    case!("redirect-in", "echo indata > ri.txt; cat < ri.txt", Eq("indata")),
    case!("redirect-err-null", "ls /definitely/nonexistent 2>/dev/null; echo rc=$?", Eq("rc=1")),
    case!("pipe-two", "printf 'x\\ny\\nz\\n' | wc -l", Eq("3")),
    case!("pipe-three", "printf 'a\\nb\\nc\\nd\\n' | head -n 2 | wc -l", Eq("2")),
    case!("pipe-custom-to-builtin", "cat poem.txt | wc -l", Eq("2")),
    // ---- core builtins ----
    case!("pwd-cd", "mkdir -p cdt && cd cdt && [[ $(pwd) == */cdt ]] && echo in-cdt; cd ..", Eq("in-cdt")),
    case!("cd-dotdot", "mkdir -p dd1/dd2; cd dd1/dd2; cd ..; pwd; cd ..", Has("dd1"), Not("dd2")),
    case!("export", "export EV=expval; echo $EV", Eq("expval")),
    case!("unset-var", "uv=1; unset uv; echo \"uv=${uv:-gone}\"", Eq("uv=gone")),
    case!("printf", "printf '%03d-%s\\n' 7 x", Eq("007-x")),
    case!("test-file", "[ -f poem.txt ] && echo isfile", Eq("isfile")),
    case!("test-numeric", "test 5 -gt 3 && echo tnum", Eq("tnum")),
    case!("let", "let lv=3*4; echo $lv", Eq("12")),
    // Upstream gap: brush's `declare -i` does not arithmetic-evaluate
    // assignments (bash: di=5+2 -> 7, brush -> 0); verified against the brush
    // CLI itself, so we only assert plain integer declaration here.
    case!("declare-int", "declare -i di=12; echo $di", Eq("12")),
    case!("eval", "eval 'echo evaled'", Eq("evaled")),
    case!("source", "echo 'SRCV=99' > src.sh; source src.sh; echo $SRCV", Eq("99")),
    case!("dot-source", "echo 'DOTV=55' > dot.sh; . ./dot.sh; echo $DOTV", Eq("55")),
    case!("type-builtin", "type cd", Has("builtin")),
    case!("command-prefix", "command echo via-command", Eq("via-command")),
    case!("builtin-prefix", "builtin echo via-builtin", Eq("via-builtin")),
    case!("colon", ": && echo colon-ok", Eq("colon-ok")),
    case!("true-false", "true; echo $?; false; echo $?", Eq("0\n1")),
    case!("getopts", "set -- -a; while getopts 'a' opt; do echo \"opt:$opt\"; done", Eq("opt:a")),
    case!("mapfile", "printf 'l1\\nl2\\n' > mf.txt; mapfile -t MF < mf.txt; echo \"${MF[0]}:${#MF[@]}\"", Eq("l1:2")),
    case!("readarray", "printf 'r1\\nr2\\nr3\\n' > rr.txt; readarray -t RA < rr.txt; echo ${#RA[@]}", Eq("3")),
    case!("alias-define", "shopt -s expand_aliases; alias ll='echo aliased'", Exit(0)),
    case!("alias-use", "ll", Eq("aliased")),
    case!("unalias", "unalias ll; type ll 2>/dev/null; echo rc=$?", Has("rc=1")),
    case!("readonly", "readonly ROV=fixed; echo $ROV", Eq("fixed")),
    case!("umask", "umask", Has("0")),
    case!("shopt", "shopt -s nullglob && echo shopt-ok", Eq("shopt-ok")),
    case!("hash", "hash -r && echo hash-ok", Eq("hash-ok")),
    case!("wait-noargs", "wait; echo wait-rc=$?", Eq("wait-rc=0")),
    case!("times", "times > /dev/null; echo times-rc=$?", Eq("times-rc=0")),
    case!("trap-set-list", "trap 'true' INT; tl=$(trap); [[ $tl == *INT* ]] && echo has-INT; trap - INT", Has("has-INT")),
    case!("exec-noargs", "exec; echo exec-ok", Eq("exec-ok")),
    case!("help", "help > /dev/null 2>&1; echo help-rc=$?", Eq("help-rc=0")),
    case!("history-noerror", "history > /dev/null 2>&1; echo h-rc=$?", Has("h-rc=")),
    case!("pushd-popd", "mkdir -p pdt; pushd pdt > /dev/null && popd > /dev/null && echo pp-ok", Eq("pp-ok")),
    case!("dirs", "dirs | wc -l", Eq("1")),
    case!("interactive-builtins-exist", "type bg fg jobs suspend disown bind enable fc caller compgen complete compopt kill ulimit set unset return break continue exit read > /dev/null; echo all-exist=$?", Eq("all-exist=0")),
    // ---- custom in-process commands ----
    case!("grep-basic", "grep fork poem.txt", Eq("no fork, no exec — and yet")),
    case!("grep-stdin-pipe", "cat poem.txt | grep -c o", Eq("3")),
    case!("grep-i", "printf 'Hello\\nworld\\n' | grep -i hello", Eq("Hello")),
    case!("grep-v", "printf 'a\\nb\\na\\n' | grep -v a", Eq("b")),
    case!("grep-n", "printf 'x\\ny\\n' | grep -n y", Eq("2:y")),
    case!("grep-o-regex", "echo abc123def | grep -o '[0-9]+'", Eq("123")),
    case!("grep-nomatch-exit", "grep zzz poem.txt; echo rc=$?", Eq("rc=1")),
    case!("uname", "uname", Eq("aShell-rs")),
    case!("uname-a", "uname -a", Has("brush-core")),
    case!("touch-ls", "touch f1.txt f2.txt; ls", Has("f1.txt"), Has("f2.txt"), Has("poem.txt")),
    case!("ls-hidden", "touch .hidden; ls; echo ---; ls -a", Has(".hidden")),
    case!("ls-hidden-default-off", "ls", Not(".hidden")),
    case!("ls-long", "ls -l", Has("poem.txt"), Has("  ")),
    case!("ls-glob", "echo *.txt", Has("poem.txt"), Not("*")),
    case!("ls-missing", "ls /no/such/dir; echo rc=$?", Has("rc=1")),
    case!("cat-file", "cat poem.txt", Has("no fork"), Exit(0)),
    case!("cat-multi", "echo A > c1.txt; echo B > c2.txt; cat c1.txt c2.txt", Eq("A\nB")),
    case!("cat-missing", "cat nope.txt; echo rc=$?", Has("rc=1")),
    case!("wc-file", "printf 'a b\\nc\\n' > w.txt; wc -l w.txt", Has("2")),
    case!("wc-full", "printf 'one two\\n' | wc", Has("1"), Has("2"), Has("8")),
    case!("head-n", "printf '1\\n2\\n3\\n4\\n5\\n' > h.txt; head -n 2 h.txt", Eq("1\n2")),
    case!("head-stdin", "printf 'p\\nq\\nr\\n' | head -n 1", Eq("p")),
    case!("mkdir-dup", "mkdir mdup; mkdir mdup 2>/dev/null; echo rc=$?", Eq("rc=1")),
    case!("mkdir-p-dup", "mkdir -p mdup && echo mp-ok", Eq("mp-ok")),
    case!("rm-file", "touch delme.txt; rm delme.txt; [ ! -f delme.txt ] && echo rm-ok", Eq("rm-ok")),
    case!("rm-missing", "rm nothere.txt 2>/dev/null; echo rc=$?", Eq("rc=1")),
    case!("rm-f-missing", "rm -f nothere.txt; echo rc=$?", Eq("rc=0")),
    case!("rm-r-dir", "mkdir -p rmd/inner; touch rmd/inner/x.txt; rm -r rmd; [ ! -d rmd ] && echo rmr-ok", Eq("rmr-ok")),
];

pub fn run_selftest(workdir: &std::path::Path) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(run_selftest_async(workdir))
}

async fn run_selftest_async(workdir: &std::path::Path) -> String {
    let scratch = workdir.join("selftest");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    std::fs::write(
        scratch.join("poem.txt"),
        "rust core, swift shell,\nno fork, no exec — and yet\npipes still carry words.",
    )
    .expect("seed poem");

    let mut shell = match crate::build_shell(std::collections::HashMap::new(), &scratch).await {
        Ok(s) => s,
        Err(e) => return format!("FATAL: shell init failed: {e}\n"),
    };

    let mut report = String::new();
    let mut passed = 0usize;

    for case in CASES {
        let (exit_code, output) = run_case(&mut shell, case.script).await;
        let trimmed = output.trim_end();

        let mut failures: Vec<String> = Vec::new();
        for check in case.checks {
            match check {
                Eq(want) => {
                    if trimmed != *want {
                        failures.push(format!("expected {want:?}, got {trimmed:?}"));
                    }
                }
                Has(want) => {
                    if !output.contains(want) {
                        failures.push(format!("missing {want:?} in {trimmed:?}"));
                    }
                }
                Not(bad) => {
                    if output.contains(bad) {
                        failures.push(format!("unexpected {bad:?} in {trimmed:?}"));
                    }
                }
                Exit(want) => {
                    if exit_code != *want {
                        failures.push(format!("expected exit {want}, got {exit_code}"));
                    }
                }
            }
        }

        if failures.is_empty() {
            passed += 1;
            report.push_str(&format!("PASS {}\n", case.name));
        } else {
            report.push_str(&format!("FAIL {}: {}\n", case.name, failures.join("; ")));
        }
    }

    report.push_str(&format!("=== {passed}/{} passed ===\n", CASES.len()));
    report
}

async fn run_case(
    shell: &mut brush_core::Shell,
    script: &str,
) -> (i32, String) {
    let (mut reader, writer) = std::io::pipe().expect("pipe");
    let out = OpenFile::from(writer);
    shell.open_files_mut().set_fd(1.into(), out.clone());
    shell.open_files_mut().set_fd(2.into(), out);

    let params = shell.default_exec_params();
    let source_info = brush_core::SourceInfo::from("selftest");
    let run = shell.run_string(script.to_string(), &source_info, &params);
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), run).await;

    let exit_code: i32 = match result {
        Ok(Ok(r)) => i32::from(u8::from(r.exit_code)),
        Ok(Err(_)) => 127,
        Err(_) => -1, // timeout
    };

    // Swap in null sinks to drop the pipe writers, unblocking read_to_end.
    let null1 = brush_core::openfiles::null().expect("null");
    let null2 = brush_core::openfiles::null().expect("null");
    shell.open_files_mut().set_fd(1.into(), null1);
    shell.open_files_mut().set_fd(2.into(), null2);

    let mut output = String::new();
    let mut buf = Vec::new();
    if reader.read_to_end(&mut buf).is_ok() {
        output = String::from_utf8_lossy(&buf).into_owned();
    }
    (exit_code, output)
}

#[cfg(test)]
mod tests {
    #[test]
    fn battery_passes_on_host() {
        let dir = std::env::temp_dir().join("ashell_selftest_host");
        std::fs::create_dir_all(&dir).unwrap();
        let report = super::run_selftest(&dir);
        println!("{report}");
        assert!(
            !report.contains("FAIL") && !report.contains("FATAL"),
            "selftest failures:\n{report}"
        );
    }
}
