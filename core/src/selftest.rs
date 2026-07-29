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
    case!("redirect-err-null", "ls /definitely/nonexistent 2>/dev/null; echo rc=$?", Eq("rc=2")),
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
    case!("typeset", "typeset TS=typed; echo $TS", Eq("typed")),
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
    case!("help", "help > /dev/null 2>&1; echo help-rc=$?", Eq("help-rc=0")),
    case!("history-noerror", "history > /dev/null 2>&1; echo h-rc=$?", Has("h-rc=")),
    case!("pushd-popd", "mkdir -p pdt; pushd pdt > /dev/null && popd > /dev/null && echo pp-ok", Eq("pp-ok")),
    case!("dirs", "dirs | wc -l", Eq("1")),
    case!("interactive-builtins-exist", "type bg fg jobs bind enable fc caller compgen complete compopt kill ulimit set unset return break continue exit read > /dev/null; echo all-exist=$?", Eq("all-exist=0")),
    case!(
        "unsupported-builtins-absent",
        "for c in disown logout exec suspend; do if type \"$c\" >/dev/null 2>&1; then echo present:$c; fi; done; echo done",
        Eq("done")
    ),
    // ---- custom in-process commands ----
    case!("grep-basic", "grep fork poem.txt", Eq("no fork, no exec — and yet")),
    case!("egrep-alias", "printf 'ab\nac\n' | egrep 'a[bc]'", Eq("ab\nac")),
    case!("fgrep-alias", "printf 'a+b\nab\n' | fgrep 'a+b'", Eq("a+b")),
    case!("grep-stdin-pipe", "cat poem.txt | grep -c o", Eq("3")),
    case!("grep-i", "printf 'Hello\\nworld\\n' | grep -i hello", Eq("Hello")),
    case!("grep-v", "printf 'a\\nb\\na\\n' | grep -v a", Eq("b")),
    case!("grep-n", "printf 'x\\ny\\n' | grep -n y", Eq("2:y")),
    // `-o` with each of grep's three pattern syntaxes. The BRE cases are here
    // because the previous grep was a hand-written CLI over Rust's `regex`,
    // which is always ERE-shaped: it had no way to express BRE, so it matched
    // `[0-9]+` where real grep does not, and this case was written to assert
    // that wrong answer. Keep all three so a future engine swap cannot quietly
    // collapse the distinction again.
    case!("grep-o-ere", "echo abc123def | grep -oE '[0-9]+'", Eq("123")),
    case!("grep-o-bre-escaped", r"echo abc123def | grep -o '[0-9]\+'", Eq("123")),
    // Default syntax is BRE, where `+` is a literal — so this must NOT match.
    case!(
        "grep-o-bre-literal-plus",
        "echo abc123def | grep -o '[0-9]+'; echo rc=$?",
        Eq("rc=1")
    ),
    case!("grep-nomatch-exit", "grep zzz poem.txt; echo rc=$?", Eq("rc=1")),
    // Capabilities Rust's `regex` cannot express at all, so these could not have
    // passed before the switch to uutils/grep (oniguruma-backed): a
    // backreference, and a PCRE lookahead.
    case!("grep-backreference", r"printf 'abab\nabcd\n' | grep '\(ab\)\1'", Eq("abab")),
    case!("grep-P-lookahead", "printf 'foo1\\nfoo2\\n' | grep -P 'foo(?=1)'", Eq("foo1")),
    case!(
        "grep-L-files-without-match",
        "printf 'x\\n' > gl1.txt; printf 'y\\n' > gl2.txt; grep -L x gl1.txt gl2.txt",
        Eq("gl2.txt")
    ),
    // GNU flag surface. These exist because a *partial* grep is worse than no
    // grep: the caller (an agent) writes the flags every other Unix accepts, and
    // a rejected flag makes it retry variations instead of changing approach.
    // `grep -iE` silently failing this way killed a whole selftest run once —
    // it exited at once, leaving its upstream writing to a reader-less pipe.
    case!("grep-E", "printf 'a\\nb\\n' | grep -E 'a|b' | wc -l", Has("2")),
    case!("grep-F-meta", "printf 'a.c\\nabc\\n' | grep -F 'a.c'", Eq("a.c")),
    case!("grep-q-exit", "printf 'x\\n' | grep -q x; echo rc=$?", Eq("rc=0")),
    case!("grep-q-silent", "printf 'x\\n' | grep -q x | wc -c", Has("0")),
    case!("grep-e-multi", "printf 'a\\nb\\nc\\n' | grep -e a -e c | wc -l", Has("2")),
    case!("grep-x", "printf 'ab\\nabc\\n' | grep -x ab", Eq("ab")),
    case!("grep-m", "printf 'a\\na\\na\\n' | grep -m1 a | wc -l", Has("1")),
    case!("grep-w", "printf 'cat\\nconcat\\n' | grep -w cat", Eq("cat")),
    case!(
        "grep-context",
        "printf '1\\n2\\nHIT\\n4\\n5\\n' | grep -C1 HIT | wc -l",
        Has("3")
    ),
    case!(
        "grep-A-B",
        "printf '1\\nHIT\\n3\\n' | grep -A1 HIT | tail -1",
        Eq("3")
    ),
    case!(
        "grep-recursive",
        "rm -rf gtree && mkdir -p gtree/sub && echo needle > gtree/sub/a.txt && grep -r needle gtree | wc -l",
        Has("1")
    ),
    case!(
        "grep-include",
        "rm -rf gi && mkdir gi && echo needle > gi/a.rs && echo needle > gi/b.txt && grep -r --include='*.rs' needle gi | wc -l",
        Has("1")
    ),
    case!(
        "grep-exclude",
        "rm -rf ge && mkdir ge && echo needle > ge/a.rs && echo needle > ge/b.txt && grep -r --exclude='*.txt' needle ge | wc -l",
        Has("1")
    ),
    case!(
        "grep-l-L",
        "rm -rf gl && mkdir gl && echo yes > gl/hit.txt && echo no > gl/miss.txt && grep -rl yes gl",
        Has("hit.txt")
    ),
    case!(
        "grep-long-opts",
        "printf 'A\\n' | grep --ignore-case --line-number a",
        Eq("1:A")
    ),
    // `-h` must stay `--no-filename` (clap would otherwise claim it for help).
    case!(
        "grep-h-is-no-filename",
        "rm -rf gh && mkdir gh && echo hit > gh/a.txt && echo hit > gh/b.txt && grep -rh hit gh | wc -l",
        Has("2")
    ),
    // --- rg (ripgrep proper) --------------------------------------------
    // Registered next to `grep`, not instead of it: rg recurses by default and
    // honours .gitignore/hidden-file rules, so the two must not be aliased.
    // These cases pin exactly those differences.
    case!(
        "rg-recursive-by-default",
        "rm -rf rgt && mkdir -p rgt/sub && echo needle > rgt/sub/a.txt && (cd rgt && rg --no-heading needle | wc -l)",
        Has("1")
    ),
    case!(
        "rg-relative-paths",
        "rm -rf rgp && mkdir -p rgp/sub && echo needle > rgp/sub/a.txt && (cd rgp && rg --no-heading -l needle)",
        Eq("sub/a.txt")
    ),
    case!(
        "rg-respects-gitignore",
        "rm -rf rgi && mkdir rgi && (cd rgi && git init -q . && echo skip.txt > .gitignore && echo needle > skip.txt && echo needle > keep.txt && rg --no-heading -l needle)",
        Eq("keep.txt")
    ),
    case!(
        "rg-no-ignore-sees-it",
        "(cd rgi && rg --no-ignore --no-heading -l needle | wc -l)",
        Has("2")
    ),
    case!(
        "rg-skips-hidden",
        "rm -rf rgh && mkdir rgh && (cd rgh && echo needle > .secret && echo needle > plain.txt && rg --no-heading -l needle)",
        Eq("plain.txt")
    ),
    case!(
        "rg-hidden-flag",
        "(cd rgh && rg --hidden --no-heading -l needle | wc -l)",
        Has("2")
    ),
    case!(
        "rg-glob",
        "rm -rf rgg && mkdir rgg && (cd rgg && echo needle > a.rs && echo needle > b.txt && rg -g '*.rs' --no-heading -l needle)",
        Eq("a.rs")
    ),
    case!(
        "rg-glob-negated",
        "(cd rgg && rg -g '!*.txt' --no-heading -l needle)",
        Eq("a.rs")
    ),
    case!(
        "rg-type",
        "(cd rgg && rg -t rust --no-heading -l needle)",
        Eq("a.rs")
    ),
    case!("rg-stdin", "printf 'a\\nb\\n' | rg b", Eq("b")),
    case!("rg-count", "(cd rgg && rg -c needle | wc -l)", Has("2")),
    case!("rg-quiet-exit", "printf 'x\\n' | rg -q x; echo rc=$?", Eq("rc=0")),
    case!(
        "rg-nomatch-exit",
        "printf 'x\\n' | rg zzz; echo rc=$?",
        Eq("rc=1")
    ),
    case!("rg-fixed-strings", "printf 'a.c\\nabc\\n' | rg -F 'a.c'", Eq("a.c")),
    case!(
        "rg-smart-case",
        "printf 'Hello\\n' | rg -S hello",
        Eq("Hello")
    ),
    case!(
        "rg-smart-case-respects-upper",
        "printf 'hello\\n' | rg -S Hello; echo rc=$?",
        Has("rc=1")
    ),
    case!(
        "rg-replace",
        "printf 'foo bar\\n' | rg -r baz foo",
        Eq("baz bar")
    ),
    case!(
        "rg-context",
        "printf '1\\nHIT\\n3\\n' | rg -A1 HIT | wc -l",
        Has("2")
    ),
    case!(
        "rg-files",
        "(cd rgg && rg --files | wc -l)",
        Has("2")
    ),
    case!(
        "rg-max-depth",
        "rm -rf rgd && mkdir -p rgd/a/b && echo needle > rgd/a/b/deep.txt && (cd rgd && rg --max-depth 1 --no-heading -l needle | wc -l)",
        Has("0")
    ),
    case!("uname", "uname", Has("Darwin")),
    case!("clear", "clear -x | wc -c", Has("7")),
    case!("clear-scrollback", "clear | od -c | head -1", Has("033")),
    // ---- ext commands (crate-backed) ----
    case!("which-builtin", "which cd", Has("builtin")),
    case!("which-cmd", "which grep 2>&1 | head -1", Has("grep")),
    case!(
        "which-missing",
        "which definitely_missing_command 2>&1; echo rc=$?",
        Has("not found"),
        Has("rc=1")
    ),
    case!(
        "which-silent",
        "which -s cd; echo found=$?; which -s definitely_missing; echo missing=$?",
        Eq("found=0\nmissing=1")
    ),
    case!(
        "which-all-path",
        "mkdir -p wp1 wp2; touch wp1/tool wp2/tool; PATH=\"$PWD/wp1:$PWD/wp2\" which -a --skip-functions tool | wc -l",
        Eq("2")
    ),
    case!(
        "interactive-adapter-aliases",
        "type edit vi nano ssh scp sftp mosh open openurl pbcopy pbpaste >/dev/null; echo aliases=$?",
        Eq("aliases=0")
    ),
    case!("stat-format", "printf x > st.txt; stat -c '%n %s' st.txt", Eq("st.txt 1")),
    // --- find / xargs (uutils/findutils, real GNU syntax) ----------------
    // NOTE: these used to be written as `find ft --name '*.txt'` — a
    // double-dash spelling the old 3-flag shim invented, which no real find
    // accepts. The tests had been bent to fit the broken CLI. Single-dash GNU
    // predicates are what an agent actually types, so that is what is pinned.
    case!(
        "find-name",
        "rm -rf ft; mkdir -p ft/sub; touch ft/a.txt ft/sub/b.txt ft/c.log; find ft -name '*.txt' | sort | tr '\\n' ' '",
        Has("a.txt"), Has("b.txt"), Not("c.log")
    ),
    case!("find-type-d", "rm -rf fd; mkdir -p fd/x fd/y; find fd -type d | wc -l", Has("3")),
    case!("find-type-f", "rm -rf ff; mkdir -p ff; touch ff/a ff/b; find ff -type f | wc -l", Has("2")),
    case!("find-maxdepth", "rm -rf md; mkdir -p md/1/2/3; find md -type d -maxdepth 1 | wc -l", Has("2")),
    case!(
        "find-iname",
        "rm -rf fi; mkdir fi; touch fi/README.md; find fi -iname 'readme*' | wc -l",
        Has("1")
    ),
    case!(
        "find-not",
        "rm -rf fn; mkdir fn; touch fn/a.txt fn/b.log; find fn -type f ! -name '*.log' | wc -l",
        Has("1")
    ),
    case!(
        "find-or",
        "rm -rf fo; mkdir fo; touch fo/a.rs fo/b.py fo/c.txt; find fo \\( -name '*.rs' -o -name '*.py' \\) | wc -l",
        Has("2")
    ),
    // GNU `-size` rounds UP to whole units, so a 10-byte file is already 1k and
    // only the empty file is "< 1k". Pinning the real semantics, not the
    // intuitive-but-wrong one.
    case!(
        "find-size",
        "rm -rf fs; mkdir fs; printf '0123456789' > fs/big; touch fs/empty; find fs -type f -size -1k | wc -l",
        Has("1")
    ),
    case!(
        "find-print0-null",
        "rm -rf fp; mkdir fp; touch fp/a; find fp -type f -print0 | tr '\\0' 'Z'",
        Has("Z")
    ),
    case!(
        "find-newer-mtime",
        "rm -rf fm; mkdir fm; touch fm/a; find fm -type f -mtime -1 | wc -l",
        Has("1")
    ),
    // -exec has no fork/exec on iOS; the vendored crate routes it through the
    // exec hook, which runs the argv in an in-process subshell.
    case!(
        "find-exec",
        "rm -rf fe; mkdir fe; touch fe/a.txt; find fe -name '*.txt' -exec echo FOUND {} \\;",
        Has("FOUND"), Has("a.txt")
    ),
    case!(
        "find-exec-plus",
        "rm -rf fep; mkdir fep; touch fep/a fep/b; find fep -type f -exec echo BATCH {} +",
        Has("BATCH")
    ),
    // xargs is 100% about running commands, so it only exists at all thanks to
    // the exec hook.
    case!("xargs-basic", "echo one two | xargs echo GOT", Has("GOT one two")),
    case!("xargs-n1", "printf 'a\\nb\\n' | xargs -n1 echo X | wc -l", Has("2")),
    case!("xargs-null", "printf 'a\\0b\\0' | xargs -0 echo | tr '\\n' ' '", Has("a b")),
    case!("xargs-replace", "echo hi | xargs -I{} echo '[{}]'", Has("[hi]")),
    case!(
        "xargs-no-run-if-empty",
        "printf '' | xargs -r echo SHOULD-NOT-APPEAR; echo done",
        Has("done"), Not("SHOULD-NOT-APPEAR")
    ),
    case!(
        "find-xargs-pipeline",
        "rm -rf fx; mkdir fx; touch fx/a.txt fx/b.txt; find fx -name '*.txt' -print0 | xargs -0 -n1 echo ITEM | wc -l",
        Has("2")
    ),
    case!("tree-basic", "mkdir -p tr/a tr/b; touch tr/a/f.txt; tree tr", Has("a"), Has("f.txt"), Has("directories")),
    case!(
        "tree-options",
        "rm -rf tro; mkdir -p tro/keep/deep tro/drop; touch tro/.hidden tro/keep/a.rs tro/keep/b.txt tro/drop/c.rs; tree -a -f -i -L 2 -P '*.rs' -I drop tro",
        Has("tro/keep"),
        Has("tro/keep/a.rs"),
        Not("b.txt"),
        Not("drop/c.rs")
    ),
    case!(
        "tree-directories-only",
        "tree -d tro",
        Has("keep"),
        Has("deep"),
        Not("a.rs")
    ),
    case!(
        "tree-missing-root",
        "tree definitely-missing-tree-root",
        Has("0 directories, 0 files")
    ),
    // --- diff / cmp (uutils/diffutils) -----------------------------------
    // The old 1-flag hand-written diff emitted unified output by DEFAULT, and
    // this test was written to match it. Real diff defaults to normal format
    // (`2c2` / `< b` / `> X`) and only produces `-b`/`+X` under `-u`. Another
    // case of the test having been bent to fit a wrong implementation.
    case!(
        "diff-normal-default",
        "printf 'a\\nb\\nc\\n' > d1.txt; printf 'a\\nX\\nc\\n' > d2.txt; diff d1.txt d2.txt; echo rc=$?",
        Has("2c2"), Has("< b"), Has("> X"), Has("rc=1")
    ),
    case!(
        "diff-same",
        "printf 'a\\nb\\n' > s1.txt; cp s1.txt s2.txt; diff s1.txt s2.txt; echo rc=$?",
        Eq("rc=0")
    ),
    // Flags the previous implementation rejected outright. `-u` is the one
    // every downstream tool (patch, git apply, review UIs) expects.
    case!(
        "diff-unified",
        "diff -u d1.txt d2.txt",
        Has("---"), Has("+++"), Has("@@"), Has("-b"), Has("+X")
    ),
    case!("diff-context", "diff -c d1.txt d2.txt", Has("***"), Has("---")),
    case!("diff-brief", "diff -q d1.txt d2.txt", Has("differ")),
    case!("diff-report-identical", "diff -s s1.txt s2.txt", Has("identical")),
    case!("diff-ed-format", "diff -e d1.txt d2.txt", Has("2c")),
    // GNU renders a substituted line as one `|` row; uutils splits it into `<`
    // and `>` rows. Assert on the column markers, not GNU's exact rendering.
    case!("diff-side-by-side", "diff -y d1.txt d2.txt", Has("<"), Has(">")),
    case!("diff-unified-lines", "diff -U1 d1.txt d2.txt", Has("@@")),
    // KNOWN GAP: uutils/diffutils 0.5.0 implements the output formats and
    // -q/-s/-t/-U/-C, but NOT the comparison-tuning flags GNU has: -i
    // (ignore-case), -w/-b (whitespace), -B (blank lines), -r (recursive),
    // -N (treat absent as empty). `diff -i a b` currently fails with
    // "Unknown option". Upstream is an early reimplementation; this is the sort
    // of hole the flag-coverage audit is meant to keep visible rather than let
    // it be discovered by an agent mid-task.
    //
    // cmp did not exist at all before.
    case!("cmp-same", "cmp s1.txt s2.txt; echo rc=$?", Eq("rc=0")),
    case!("cmp-differ", "cmp d1.txt d2.txt", Has("differ")),
    case!("cmp-silent", "cmp -s d1.txt d2.txt; echo rc=$?", Eq("rc=1")),
    case!("cmp-verbose", "cmp -l d1.txt d2.txt | head -1", Has("2")),
    case!("gzip-roundtrip", "rm -f gz.txt gz.txt.gz; echo 'compress me please' > gz.txt; gzip gz.txt; [ -f gz.txt.gz ] && gunzip gz.txt.gz && cat gz.txt", Eq("compress me please")),
    case!("gzip-stdin-pipe", "echo hello | gzip -c | gunzip -c", Eq("hello")),
    case!("sed-substitute", "printf 'foo bar\\nfoo baz\\n' | sed 's/foo/XXX/'", Eq("XXX bar\nXXX baz")),
    case!("sed-global", "echo 'a a a' | sed 's/a/b/g'", Eq("b b b")),
    case!("sed-e-multi", "echo hello | sed -e 's/h/H/' -e 's/o/O/'", Eq("HellO")),
    case!("sed-empty-line", "printf 'a\\n\\nb\\n' | sed 's/^/X/'", Eq("Xa\nX\nXb")),
    // Flags the previous 2-flag hand-written sed simply rejected. `-i` and `-n`
    // in particular are what an agent reaches for first.
    case!(
        "sed-in-place",
        "printf 'foo\\n' > si.txt && sed -i 's/foo/bar/' si.txt && cat si.txt",
        Eq("bar")
    ),
    case!("sed-quiet-print", "printf 'a\\nb\\nc\\n' | sed -n '2p'", Eq("b")),
    case!("sed-extended-regex", "echo abc123 | sed -E 's/[0-9]+/N/'", Eq("abcN")),
    case!("sed-delete", "printf 'a\\nb\\nc\\n' | sed '2d' | tr '\\n' ' '", Has("a c")),
    case!(
        "sed-script-file",
        "printf 's/x/Y/\\n' > sf.sed && echo x | sed -f sf.sed",
        Eq("Y")
    ),
    case!("sed-line-range", "printf '1\\n2\\n3\\n4\\n' | sed -n '2,3p' | tr '\\n' ' '", Has("2 3")),
    case!("sed-insert-text", "printf 'x\\n' | sed '1i\\\ntop'", Has("top"), Has("x")),
    // ---- curl official upstream CLI ----
    case!("curl-version", "curl --version | head -n 1", Has("curl 8.1.2")),
    case!("curl-file", "printf upstream-curl > curl-local.txt; curl -s \"file://$PWD/curl-local.txt\"", Eq("upstream-curl")),
    case!("curl-output", "curl -s -o curl-copy.txt \"file://$PWD/curl-local.txt\"; cat curl-copy.txt", Eq("upstream-curl")),
    case!("curl-repeat", "curl -s \"file://$PWD/curl-local.txt\"; echo; curl -s \"file://$PWD/curl-local.txt\"", Eq("upstream-curl\nupstream-curl")),
    case!("wget-version", "wget --version", Has("curl 8.1.2")),
    case!("wget-default-output", "mkdir wget-src; printf upstream-wget > wget-src/payload.txt; wget -q \"file://$PWD/wget-src/payload.txt\"; cat payload.txt", Eq("upstream-wget")),
    case!("wget-stdout", "wget -q -O - \"file://$PWD/wget-src/payload.txt\"", Eq("upstream-wget")),
    case!("wget-directory-prefix", "wget -q -P downloads \"file://$PWD/wget-src/payload.txt\"; cat downloads/payload.txt", Eq("upstream-wget")),
    case!("wget-output-equals", "wget -q --output-document=equals.txt \"file://$PWD/wget-src/payload.txt\"; cat equals.txt", Eq("upstream-wget")),
    case!("wget-common-options", "wget -q -4 --connect-timeout=2 --max-redirect=3 --user-agent=YourShell --referer=https://example.invalid/ -O common.txt \"file://$PWD/wget-src/payload.txt\"; cat common.txt", Eq("upstream-wget")),
    case!("tar-roundtrip", "mkdir -p td/x; echo content > td/x/f.txt; tar -czf t.tgz td; rm -rf td; tar -xzf t.tgz; cat td/x/f.txt", Eq("content")),
    case!("tar-list", "mkdir -p tl; echo a > tl/a.txt; tar -cf tl.tar tl; tar -tf tl.tar | grep -c txt", Has("1")),
    case!("tar-bzip2", "mkdir tj; echo bz > tj/f; tar -cjf tj.tar.bz2 tj; rm -rf tj; tar -xjf tj.tar.bz2; cat tj/f", Eq("bz")),
    case!("tar-xz", "mkdir tx; echo xz > tx/f; tar -cJf tx.tar.xz tx; rm -rf tx; tar -xJf tx.tar.xz; cat tx/f", Eq("xz")),
    case!("tar-stdin-stdout", "mkdir ts; echo stream > ts/f; tar -cf - ts | tar -tf - | grep -c 'ts/f'", Eq("1")),
    case!("tar-exclude-strip", "mkdir -p te/root; echo yes > te/root/a; echo no > te/root/b; tar -cf te.tar --exclude='*/b' te/root; mkdir tout; tar -xf te.tar -C tout --strip-components 2; cat tout/a; test ! -e tout/b", Eq("yes")),
    case!("tar-repeat-after-error", "tar --definitely-invalid >/dev/null 2>&1 || :; rm -rf tr tr.tar; mkdir tr; echo again > tr/f; tar -cf tr.tar tr; tar -tf tr.tar | grep -c tr/f", Eq("1")),
    case!("zip-unzip-roundtrip", "mkdir -p zt; echo zipped > zt/z.txt; zip -r z.zip zt > /dev/null; rm -rf zt; unzip z.zip > /dev/null; cat zt/z.txt", Eq("zipped")),
    case!(
        "zip-verbose-add",
        "echo verbose > zv.txt; zip zv.zip zv.txt",
        Has("adding: zv.txt")
    ),
    case!("unzip-list", "echo hi > u.txt; zip u.zip u.txt > /dev/null; unzip -l u.zip | grep -c u.txt", Has("1")),
    case!("zip-junk-exclude", "rm -rf zj; mkdir -p zj/a zj/b; echo a > zj/a/a.txt; echo b > zj/b/b.tmp; zip -q -r -j -x '*.tmp' zj.zip zj; unzip -p zj.zip a.txt", Eq("a")),
    case!("zip-update", "echo old > zu.txt; zip -q zu.zip zu.txt; echo new > zu.txt; zip -q -u zu.zip zu.txt; unzip -p zu.zip zu.txt", Eq("new")),
    case!(
        "zip-update-verbose-carried",
        "echo one > zuc1; echo two > zuc2; zip -q zuc.zip zuc1 zuc2; echo changed > zuc1; zip -u zuc.zip zuc1; unzip -p zuc.zip zuc2",
        Eq("  adding: zuc1\ntwo")
    ),
    case!("zip-delete", "echo keep > zd1; echo drop > zd2; zip -q zd.zip zd1 zd2; zip -q -d zd.zip zd2; unzip -l zd.zip | grep -c zd2", Eq("0")),
    case!(
        "zip-delete-verbose",
        "echo remove > zdv; zip -q zdv.zip zdv; zip -d zdv.zip zdv",
        Has("deleting: zdv")
    ),
    case!("zip-store", "echo stored > zs.txt; zip -q -0 zs.zip zs.txt; unzip -p zs.zip zs.txt", Eq("stored")),
    case!("zip-names-stdin", "echo stdin > zi.txt; printf 'zi.txt\n' | zip -q zi.zip -@; unzip -p zi.zip zi.txt", Eq("stdin")),
    case!("sqlite-memory", "sqlite3 :memory: 'create table t(a,b); insert into t values(1,2),(3,4); select sum(a),sum(b) from t'", Eq("4|6")),
    case!("sqlite-file", "sqlite3 test.db 'create table u(name text)'; sqlite3 test.db \"insert into u values('alice')\"; sqlite3 test.db 'select name from u'", Eq("alice")),
    case!("sqlite-stdin", "echo 'select 6*7' | sqlite3 :memory:", Eq("42")),
    case!("sqlite-header", "sqlite3 --header :memory: 'select 1 as x, 2 as y'", Eq("x|y\n1|2")),
    case!("sqlite-semicolon-in-string", "sqlite3 :memory: \"create table t(x); insert into t values('a;b'); select x from t\"", Eq("a;b")),
    // These exercise SQLite's real shell rather than just the database engine.
    // The previous rusqlite wrapper had no dot commands or output modes.
    case!("sqlite-dot-mode", "sqlite3 :memory: '.headers on' '.mode csv' 'select 1 as x, 2 as y'", Eq("x,y\r\n1,2")),
    case!("sqlite-dot-schema", "sqlite3 schema.db 'create table items(id integer, name text)'; sqlite3 schema.db '.schema items'", Has("CREATE TABLE items")),
    case!("sqlite-json-mode", "sqlite3 :memory: '.mode json' 'select 7 as n, \"ok\" as s'", Eq("[{\"n\":7,\"s\":\"ok\"}]")),
    case!("sqlite-invalid-option-contained", "sqlite3 --definitely-invalid 2>/dev/null; echo rc=$?; echo still-alive", Has("rc=1"), Has("still-alive")),
    case!("sqlite-repeat-after-exit", "sqlite3 :memory: '.exit 7'; echo first=$?; sqlite3 :memory: 'select 9'; echo second=$?", Eq("first=7\n9\nsecond=0")),
    case!("jq-field", "echo '{\"name\":\"alice\",\"age\":30}' | jq '.name'", Eq("\"alice\"")),
    case!("jq-raw", "echo '{\"name\":\"alice\"}' | jq -r '.name'", Eq("alice")),
    case!("jq-array-map", "echo '[1,2,3]' | jq 'map(. * 2)'", Has("2"), Has("4"), Has("6")),
    case!("jq-pipe", "echo '{\"a\":{\"b\":42}}' | jq '.a.b'", Eq("42")),
    case!("jq-keys", "echo '{\"x\":1,\"y\":2}' | jq -r 'keys[]'", Eq("x\ny")),
    case!("jq-select", "echo '[{\"n\":1},{\"n\":5}]' | jq '.[] | select(.n > 3) | .n'", Eq("5")),
    // ---- git (libgit2, local ops) ----
    case!("git-version", "git --version", Has("libgit2")),
    case!("git-init", "rm -rf gr; mkdir gr; (cd gr && git init 2>&1 | head -1)", Has("Initialized")),
    case!("git-config-commit-log", "(cd gr && git config user.name Tester && git config user.email t@e.com && echo hello > f.txt && git add f.txt && git commit -m 'first commit' 2>&1 | head -1 && git log -n 1 | grep -c 'first commit')", Has("1")),
    case!("git-status-clean", "(cd gr && git status | tail -1)", Has("clean")),
    case!("git-status-dirty", "(cd gr && echo more > f2.txt && git status | grep -c f2.txt)", Has("1")),
    case!("git-status-porcelain", "(cd gr && printf untracked > new.txt && git status --porcelain=v1 -b)", Has("## master"), Has("?? new.txt")),
    case!("git-branch", "(cd gr && git branch newbr && git branch | grep -c newbr)", Has("1")),
    case!("git-diff", "(cd gr && echo changed >> f.txt && git diff | grep -c changed)", Has("1")),
    case!("git-global-C", "git -C gr --version", Has("libgit2")),
    case!("git-init-branch", "mkdir git-main; git -C git-main init -q -b main; git -C git-main status", Has("On branch main")),
    case!("git-git-dir-work-tree", "mkdir git-split; git -C git-split init -q; printf split > git-split/a; git --git-dir=git-split/.git --work-tree=git-split add a; git --git-dir=git-split/.git --work-tree=git-split status", Has("new: a")),
    case!("git-init-bare", "git init -q --bare bare.git; test -f bare.git/HEAD", Eq("")),
    // ---- uutils adapter: session-state sync ----
    case!("uu-env-sync", "export UUV=fromshell; printenv UUV", Eq("fromshell")),
    case!("uu-cwd-sync", "mkdir -p cwt && cd cwt && touch inner.txt && ls && cd ..", Has("inner.txt")),
    // ---- uutils adapter: breadth ----
    case!("sort", "printf 'b\\na\\nc\\n' | sort", Eq("a\nb\nc")),
    case!("sort-rn", "printf '1\\n3\\n2\\n' | sort -rn", Eq("3\n2\n1")),
    case!("uniq", "printf 'a\\na\\nb\\n' | uniq", Eq("a\nb")),
    case!("cut", "echo 'a:b:c' | cut -d: -f2", Eq("b")),
    case!("tr", "echo abc | tr 'a-z' 'A-Z'", Eq("ABC")),
    case!("tac", "printf '1\\n2\\n3\\n' | tac", Eq("3\n2\n1")),
    case!("seq", "seq 3", Eq("1\n2\n3")),
    case!("tail", "printf '1\\n2\\n3\\n4\\n' | tail -n 2", Eq("3\n4")),
    case!("basename", "basename /some/path/file.txt", Eq("file.txt")),
    case!("dirname", "dirname /some/path/file.txt", Eq("/some/path")),
    case!("date", "date +%Y", Has("20")),
    case!("expr", "expr 6 \\* 7", Eq("42")),
    case!("factor", "factor 12", Has("2 2 3")),
    case!("sha256sum", "printf hi | sha256sum", Has("8f434346648f6b96")),
    case!("base64-cmd", "printf hi | base64", Eq("aGk=")),
    case!("basenc-base16", "printf hi | basenc --base16", Eq("6869")),
    case!("b2sum", "printf hi | b2sum | cut -c1-8", Eq("bfbcbe7a")),
    case!("cksum", "printf hi | cksum | cut -d' ' -f2", Eq("2")),
    case!("sha224sum", "printf hi | sha224sum | cut -c1-8", Eq("1a15bca3")),
    case!("sha384sum", "printf hi | sha384sum | cut -c1-8", Eq("0791006d")),
    case!("sha512sum", "printf hi | sha512sum | cut -c1-8", Eq("150a14ed")),
    case!("nl", "printf 'x\\ny\\n' | nl", Has("1"), Has("x")),
    case!("paste", "printf 'a\\nb\\n' > p1.txt; printf '1\\n2\\n' > p2.txt; paste -d: p1.txt p2.txt", Eq("a:1\nb:2")),
    case!("tee", "echo teed | tee tee.txt; cat tee.txt", Eq("teed\nteed")),
    case!("comm", "printf 'a\\nb\\n' > cm1.txt; printf 'b\\nc\\n' > cm2.txt; comm -12 cm1.txt cm2.txt", Eq("b")),
    case!("cp-mv", "echo v > cpm.txt; cp cpm.txt cpm2.txt; mv cpm2.txt cpm3.txt; cat cpm3.txt", Eq("v")),
    case!("ln-readlink", "echo tgt > lnt.txt; ln -s lnt.txt lns.txt; cat lns.txt; readlink lns.txt", Eq("tgt\nlnt.txt")),
    case!("realpath-cmd", "realpath lnt.txt", Has("/"), Has("lnt.txt")),
    case!("rmdir", "mkdir rdt; rmdir rdt; [ ! -d rdt ] && echo rmdir-ok", Eq("rmdir-ok")),
    case!("truncate", "truncate -s 5 trc.txt; wc -c trc.txt", Has("5")),
    case!("mktemp-cmd", "t=$(mktemp tmp.XXXXXX); [ -f \"$t\" ] && echo mktemp-ok; rm -f \"$t\"", Eq("mktemp-ok")),
    case!("du", "du -s . > /dev/null; echo du-rc=$?", Eq("du-rc=0")),
    case!("df", "df .", Has("/")),
    case!("dir", "dir poem.txt", Has("poem.txt")),
    case!("vdir", "vdir poem.txt", Has("poem.txt")),
    case!("dircolors", "dircolors -b", Has("LS_COLORS")),
    case!("env", "env UUTEST=works", Has("UUTEST=works")),
    case!("whoami-nproc-hostname", "whoami > /dev/null && nproc > /dev/null && hostname > /dev/null; echo id-rc=$?", Eq("id-rc=0")),
    case!("sleep-cmd", "sleep 0; echo slept=$?", Eq("slept=0")),
    case!("od", "printf 'A' | od -An -c", Has("A")),
    case!("fold", "echo abcdef | fold -w 3", Eq("abc\ndef")),
    case!("fmt", "printf 'one two three four\\n' | fmt -w 8", Eq("one two\nthree\nfour")),
    case!("pr", "printf 'line\\n' | pr -t", Eq("line")),
    case!("ptx", "printf 'alpha beta\\n' | ptx -f", Has("alpha")),
    case!("split-cmd", "printf '1\\n2\\n3\\n4\\n' > sp.txt; split -l 2 sp.txt spx_; cat spx_aa", Eq("1\n2")),
    case!("csplit", "printf 'a\\nMARK\\nb\\n' > cs.txt; csplit -s cs.txt /MARK/; cat xx01", Eq("MARK\nb")),
    case!("sum", "printf hi | sum | wc -w", Eq("2")),
    case!("shred", "printf secret > shred.txt; shred -n 1 -z shred.txt; wc -c shred.txt", Has("6")),
    case!("tsort", "printf 'a b\\nb c\\n' | tsort", Eq("a\nb\nc")),
    case!("expand-roundtrip", "printf '\\tx\\n' | expand | unexpand | wc -l", Eq("1")),
    // ---- grep (ripgrep engine) extended ----
    case!("grep-l", "echo hay > g1.txt; echo needle > g2.txt; grep -l needle g1.txt g2.txt", Eq("g2.txt")),
    case!("grep-w", "printf 'foo\\nfoobar\\n' | grep -w foo", Eq("foo")),
    case!("grep-F", "printf 'a+b\\nc\\n' | grep -F 'a+b'", Eq("a+b")),
    case!("grep-s-missing", "grep -s pat nosuch.txt; echo rc=$?", Eq("rc=2")),
    case!("grep-multifile-prefix", "printf 'x\\n' > gm1.txt; printf 'x\\n' > gm2.txt; grep x gm1.txt gm2.txt", Eq("gm1.txt:x\ngm2.txt:x")),
    case!("grep-c-multi", "grep -c x gm1.txt gm2.txt", Eq("gm1.txt:1\ngm2.txt:1")),
    // ---- shell semantics depth ----
    case!("redirect-2to1", "{ echo std; ls /nope 2>&1; } | wc -l", Eq("2")),
    case!("cmdsubst-file", "echo filecontent > cs.txt; echo $(< cs.txt)", Eq("filecontent")),
    case!("heredoc-expand", "hv=world; cat <<EOF\nhello $hv\nEOF", Eq("hello world")),
    case!("heredoc-quoted-noexpand", "cat <<'EOF'\nliteral $hv\nEOF", Eq("literal $hv")),
    case!("regex-match", "[[ abc123 =~ [0-9]+ ]] && echo re-ok", Eq("re-ok")),
    case!("regex-capture", "[[ ab12cd =~ ([0-9]+) ]] && echo ${BASH_REMATCH[1]}", Eq("12")),
    case!("case-upper", "s=hello; echo ${s^^}", Eq("HELLO")),
    case!("case-lower", "s=HELLO; echo ${s,,}", Eq("hello")),
    case!("indirect-var", "target=hello; ptr=target; echo ${!ptr}", Eq("hello")),
    case!("array-slice", "a=(1 2 3 4 5); echo ${a[@]:1:2}", Eq("2 3")),
    case!("array-append", "a=(1); a+=(2 3); echo ${#a[@]}", Eq("3")),
    case!("array-negative", "a=(x y z); echo ${a[-1]}", Eq("z")),
    case!("arith-assign-ops", "n=10; ((n += 5)); ((n *= 2)); echo $n", Eq("30")),
    case!("arith-command", "if ((3 > 2)); then echo arith-if; fi", Eq("arith-if")),
    case!("command-v", "command -v cd", Eq("cd")),
    case!("type-t", "type -t cd", Eq("builtin")),
    case!("param-prefix-strip", "f=/a/b/c.txt; echo ${f##*/} ${f%.txt}", Eq("c.txt /a/b/c")),
    case!("quoted-at", "f() { echo $#; }; f \"a b\" c", Eq("2")),
    case!("star-vs-at", "set -- a b; x=\"$*\"; echo \"$x\"", Eq("a b")),
    // ---- uutils flag depth ----
    case!("ls-R", "mkdir -p lsr/sub; touch lsr/sub/deep.txt; ls -R lsr", Has("deep.txt"), Has("sub")),
    case!("cp-r", "mkdir -p cpr/s; echo d > cpr/s/f.txt; cp -r cpr cpr2; cat cpr2/s/f.txt", Eq("d")),
    case!("rm-rf-deep", "mkdir -p rrf/a/b/c; touch rrf/a/b/c/x; rm -rf rrf; [ ! -d rrf ] && echo rrf-ok", Eq("rrf-ok")),
    case!("head-c", "printf abcdef | head -c 3", Eq("abc")),
    case!("tail-c", "printf abcdef | tail -c 2", Eq("ef")),
    case!("wc-w-c", "printf 'one two three' | wc -w; printf abc | wc -c", Eq("3\n3")),
    case!("sort-k-t", "printf 'b:2\\na:1\\n' | sort -t: -k2 -n", Eq("a:1\nb:2")),
    case!("sort-u", "printf 'b\\na\\nb\\n' | sort -u", Eq("a\nb")),
    case!("uniq-d", "printf 'a\\na\\nb\\n' | uniq -d", Eq("a")),
    case!("cut-c", "echo abcdef | cut -c1-3", Eq("abc")),
    case!("tr-d", "echo 'a-b-c' | tr -d '-'", Eq("abc")),
    case!("tr-s", "echo 'aaabbb' | tr -s ab", Eq("ab")),
    case!("seq-step", "seq 2 2 8 | paste -sd, -", Eq("2,4,6,8")),
    case!("date-epoch", "d=$(date -u +%s); [ \"$d\" -gt 1700000000 ] && echo epoch-ok", Eq("epoch-ok")),
    case!("md5-sha1", "printf hi | md5sum | cut -c1-8; printf hi | sha1sum | cut -c1-8", Eq("49f68a5c\nc22b5f91")),
    case!("base64-roundtrip", "printf secret | base64 | base64 -d", Eq("secret")),
    case!("base32-roundtrip", "printf secret | base32 | base32 -d", Eq("secret")),
    case!("dd-count", "printf 0123456789 | dd bs=2 count=2 2>/dev/null", Eq("0123")),
    case!("join-cmd", "printf '1 a\\n2 b\\n' > j1.txt; printf '1 x\\n2 y\\n' > j2.txt; join j1.txt j2.txt", Eq("1 a x\n2 b y")),
    case!("numfmt", "numfmt --to=iec 2048", Eq("2.0K")),
    case!("printenv-path", "printenv PATH | wc -l", Eq("1")),
    case!("shuf-count", "seq 5 | shuf | sort -n | paste -sd, -", Eq("1,2,3,4,5")),
    case!("link-unlink", "echo hl > hl.txt; link hl.txt hl2.txt; cat hl2.txt; unlink hl2.txt; [ ! -f hl2.txt ] && echo ul-ok", Eq("hl\nul-ok")),
    case!("split-b", "printf ABCDEF > sb.txt; split -b 2 sb.txt sbx_; cat sbx_ac", Eq("EF")),
    case!("od-hex", "printf 'A' | od -An -tx1", Has("41")),
    case!("nl-ba", "printf 'x\\n\\ny\\n' | nl -ba | wc -l", Eq("3")),
    case!("arch-sync", "arch > /dev/null && sync && echo as-ok", Eq("as-ok")),
    case!("touch-ls", "touch f1.txt f2.txt; ls", Has("f1.txt"), Has("f2.txt"), Has("poem.txt")),
    case!("ls-hidden", "touch .hidden; ls; echo ---; ls -a", Has(".hidden")),
    case!("ls-hidden-default-off", "ls", Not(".hidden")),
    case!("ls-long", "ls -l", Has("poem.txt"), Has("  ")),
    case!("ls-glob", "echo *.txt", Has("poem.txt"), Not("*")),
    case!("ls-missing", "ls /no/such/dir 2>/dev/null; echo rc=$?", Has("rc=2")),
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
    // ---- awk (one-true-awk, vendored C, in-process) ----
    case!("awk-field", "echo 'a b c' | awk '{print $2}'", Eq("b"), Exit(0)),
    case!("awk-sum", "printf '1\\n2\\n3\\n' | awk '{s+=$1} END{print s}'", Eq("6")),
    case!("awk-fs", "echo 'x,y,z' | awk -F, '{print $3}'", Eq("z")),
    case!("awk-pattern", "printf 'foo\\nbar\\n' | awk '/foo/{print}'", Eq("foo")),
];

/// Python cases run only when the `python` feature is compiled in
/// (detected at runtime via `type python3`). pip cases require network.
pub static PY_CASES: &[Case] = &[
    case!("py-version", "python3 --version", Has("Python 3.14")),
    case!("py-c", "python3 -c 'print(21 * 2)'", Eq("42")),
    case!("py-argv", "python3 -c 'import sys; print(sys.argv[1])' hello-arg", Eq("hello-arg")),
    case!("py-file", "printf 'import sys\\nprint(\"from-file\", sys.argv[1])\\n' > pys.py; python3 pys.py world", Eq("from-file world")),
    case!("py-m-module", "echo '{\"b\": 1}' | python3 -m json.tool", Has("\"b\": 1")),
    case!("py-stdin", "printf 'print(5 + 5)\\n' > si.py; python3 < si.py", Eq("10")),
    case!("py-exit-code", "python3 -c 'import sys; sys.exit(3)'; echo rc=$?", Eq("rc=3")),
    case!("py-traceback", "python3 -c '1/0' 2>&1; echo rc=$?", Has("ZeroDivisionError"), Has("rc=1")),
    case!("py-native-modules", "python3 -c 'import math, json, zlib, sqlite3, ssl, hashlib; print(\"mods-ok\")'", Eq("mods-ok")),
    case!("py-subinterp-isolation", "python3 -c 'leak = 1; print(\"first\")'; python3 -c 'print(\"leak\" in dir())'", Eq("first\nFalse")),
    case!("py-env-passthrough", "export PYE=fromshell; python3 -c 'import os; print(os.environ[\"PYE\"])'", Eq("fromshell")),
    case!("py-cwd", "python3 -c 'import os; print(os.path.basename(os.getcwd()))'", Has("selftest_")),
    case!("py-pipe-to-shell", "python3 -c 'print(chr(104)+chr(105))' | tr 'a-z' 'A-Z'", Eq("HI")),
    // iOS: ensurepip needs subprocess (unsupported) and user-site is
    // disabled, so pip is bootstrapped by unzipping the bundled wheel into
    // our writable site dir, and installs use --target.
    case!("py-pip-bootstrap", "python3 -c 'import zipfile, glob, os; w = glob.glob(os.path.join(os.environ[\"YOURSHELL_PYTHON_HOME\"], \"lib/python3.14/ensurepip/_bundled/pip-*.whl\"))[0]; zipfile.ZipFile(w).extractall(os.environ[\"YOURSHELL_PY_SITE\"]); print(\"pip-boot-ok\")'", Eq("pip-boot-ok")),
    case!("py-pip-version", "export PIP_DISABLE_PIP_VERSION_CHECK=1; python3 -m pip --version 2>&1", Has("pip 26")),
    case!("pip-direct-version", "export PIP_DISABLE_PIP_VERSION_CHECK=1; pip --version 2>&1", Has("pip 2")),
    case!("pip3-direct", "export PIP_DISABLE_PIP_VERSION_CHECK=1; pip3 --version 2>&1", Has("pip 2")),
    case!("py-pip-install", "python3 -m pip install --quiet --no-input --upgrade --only-binary :all: --target \"$YOURSHELL_PY_SITE\" six > /dev/null 2>&1; python3 -c 'import six; print(six.__name__, six.__version__ != \"\")'", Has("six True")),
    // ---- python real-world scenarios (network + binary wheels) ----
    case!("py-pip-rich", "python3 -m pip install -q --no-input --only-binary :all: --target \"$YOURSHELL_PY_SITE\" rich > /dev/null 2>&1; python3 -c 'from rich.console import Console; Console(force_terminal=False).print(\"rich\", 6*7)'", Has("rich 42")),
    case!("py-pip-fpdf2-pillow", "python3 -m pip install -q --no-input --only-binary :all: --target \"$YOURSHELL_PY_SITE\" fpdf2 pypdf requests > /dev/null 2>&1; python3 -c 'import fpdf, pypdf, requests, PIL; print(\"heavy-deps-ok\")'", Eq("heavy-deps-ok")),
    case!("py-pil-image", "python3 -c 'from PIL import Image; img = Image.new(\"RGB\", (80, 40), \"blue\"); img.save(\"blue.png\"); print(\"png\", img.size[0], img.size[1])'", Has("png 80 40")),
    case!("py-pdf-generate", "printf 'from fpdf import FPDF\\npdf = FPDF()\\npdf.add_page()\\npdf.set_font(\"Helvetica\", size=20)\\npdf.cell(text=\"YourShell PDF\")\\npdf.image(\"blue.png\", x=10, y=30, w=40)\\npdf.output(\"gen.pdf\")\\nprint(\"pdf-generated\")\\n' > genpdf.py; python3 genpdf.py; head -c 5 gen.pdf; echo", Has("pdf-generated"), Has("%PDF-")),
    case!("py-pdf-readback", "python3 -c 'from pypdf import PdfReader; r = PdfReader(\"gen.pdf\"); print(\"pages\", len(r.pages))'", Eq("pages 1")),
    case!("py-requests-https", "python3 -c 'import requests; r = requests.get(\"https://pypi.org/simple/\", timeout=15); print(\"http\", r.status_code)'", Eq("http 200")),
    case!("py-hot-search", "python3 -c 'import requests; r = requests.get(\"https://top.baidu.com/board?tab=realtime\", headers={\"User-Agent\":\"Mozilla/5.0\"}, timeout=15); print(\"hot-search\", r.status_code, len(r.text) > 1000)'", Has("hot-search 200 True")),
    case!("py-data-analysis", "python3 -m pip install -q --no-input --only-binary :all: --target \"$YOURSHELL_PY_SITE\" numpy pandas > /dev/null 2>&1; python3 -c 'import numpy as np, pandas as pd; x=np.array([1,2,3]); df=pd.DataFrame({\"x\":x}); print(\"data-analysis\", int(df.x.mean()), len(df))'", Has("data-analysis 2 3")),
    case!("py-sqlite-roundtrip", "python3 -c 'import sqlite3; c = sqlite3.connect(\"t.db\"); c.execute(\"create table if not exists kv(k, v)\"); c.execute(\"insert into kv values(?, ?)\", (\"a\", 42)); c.commit(); print(c.execute(\"select sum(v) from kv\").fetchone()[0] >= 42)'", Eq("True")),
    // python-pptx blocked upstream: lxml has no iOS wheel yet. A minimal
    // valid .pptx is assembled with stdlib zipfile+xml instead, proving the
    // OOXML container pipeline works end to end.
    // python-pptx works via the Flet iOS wheel index (lxml + libxml2).
    case!("py-pptx-real", "python3 -m pip install -q --no-input --only-binary :all: --index-url https://pypi.flet.dev --extra-index-url https://pypi.org/simple --target \"$YOURSHELL_PY_SITE\" python-pptx > /dev/null 2>&1; cat > genppt.py <<'PYSRC'\nfrom pptx import Presentation\nprs = Presentation()\nslide = prs.slides.add_slide(prs.slide_layouts[0])\nslide.shapes.title.text = 'YourShell'\nprs.save('real.pptx')\nprint('pptx-saved')\nPYSRC\npython3 genppt.py; python3 -c 'from pptx import Presentation; print(\"title:\", Presentation(\"real.pptx\").slides[0].shapes.title.text)'", Has("pptx-saved"), Has("title: YourShell")),
    // Full circle: PIL renders text, the native Vision-backed ocr command
    // reads it back.
    case!("ocr-vision-roundtrip", "cat > genocr.py <<'PYSRC'\nfrom PIL import Image, ImageDraw, ImageFont\nimg = Image.new('RGB', (900, 200), 'white')\nd = ImageDraw.Draw(img)\nfont = ImageFont.load_default(size=96)\nd.text((40, 40), 'HELLO YOURSHELL', fill='black', font=font)\nimg.save('ocr_input.png')\nprint('image-ok')\nPYSRC\npython3 genocr.py; ocr ocr_input.png", Has("image-ok"), Has("HELLO"), Has("YOURSHELL")),
];

/// Node cases run only when the `node` feature is compiled in (detected via
/// `type node`). The resident instance launches lazily on the first case; npm
/// cases require network.
pub static NODE_CASES: &[Case] = &[
    case!("node-version", "node -v", Has("v18")),
    case!("node-eval", "node -e 'console.log(6 * 7)'", Eq("42")),
    case!("node-print", "node -p '40 + 2'", Eq("42")),
    case!("node-process", "node -e 'console.log(process.platform, process.arch)'", Has("ios")),
    case!("node-require-builtin", "node -e 'const os=require(\"os\"); const p=require(\"path\"); console.log(p.join(\"a\",\"b\"), typeof os.cpus)'", Eq("a/b function")),
    case!("node-fs", "node -e 'const fs=require(\"fs\"); fs.writeFileSync(\"nf.txt\",\"ndata\"); console.log(fs.readFileSync(\"nf.txt\",\"utf8\"))'", Eq("ndata")),
    case!("node-json", "node -e 'console.log(JSON.stringify({a:1,b:[2,3]}))'", Eq("{\"a\":1,\"b\":[2,3]}")),
    case!("node-argv", "node -e 'console.log(process.argv.slice(2).join(\"-\"))' one two", Eq("one-two")),
    case!("node-exit-code", "node -e 'process.exit(5)'; echo rc=$?", Eq("rc=5")),
    case!("node-pipe-to-shell", "node -e 'console.log(\"HeLLo\")' | tr 'A-Z' 'a-z'", Eq("hello")),
    case!("node-buffer-crypto", "node -e 'const c=require(\"crypto\"); console.log(c.createHash(\"sha256\").update(\"hi\").digest(\"hex\").slice(0,8))'", Eq("8f434346")),
    case!("node-reuse", "node -e 'console.log(1)'; node -e 'console.log(2)'; node -e 'console.log(3)'", Eq("1\n2\n3")),
    case!("npm-version", "npm --version", Has("10.")),
    case!("npm-init-y", "rm -rf ~/Documents/ntest && mkdir -p ~/Documents/ntest && cd ~/Documents/ntest && npm init -y > /dev/null 2>&1 && node -e 'console.log(require(\"./package.json\").name)'", Eq("ntest")),
    case!("npm-install-require", "cd ~/Documents/ntest && npm install is-odd > /dev/null 2>&1; node -e 'console.log(require(\"is-odd\")(7), require(\"is-odd\")(8))'", Eq("true false")),
    case!("npm-install-tree", "cd ~/Documents/ntest && npm install cowsay > /dev/null 2>&1; node -e 'console.log(require(\"cowsay\").say({text:\"hi\"}).includes(\"hi\"))'", Eq("true")),
];

pub fn run_selftest(workdir: &std::path::Path) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(run_selftest_async(workdir))
}

async fn run_selftest_async(workdir: &std::path::Path) -> String {
    let scratch = workdir.join(format!("selftest_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    std::fs::write(
        scratch.join("poem.txt"),
        "rust core, swift shell,\nno fork, no exec — and yet\npipes still carry words.",
    )
    .expect("seed poem");

    // Optional incremental report path: flushed after every case so progress
    // survives if iOS kills a long background run before the battery finishes.
    let report_path = std::env::var_os("YS_SELFTEST_REPORT").map(std::path::PathBuf::from);
    let flush = |report: &str| {
        if let Some(p) = &report_path {
            let _ = std::fs::write(p, report);
        }
    };

    let mut shell = match crate::build_shell(std::collections::HashMap::new(), &scratch).await {
        Ok(s) => s,
        Err(e) => return format!("FATAL: shell init failed: {e}\n"),
    };

    let mut report = String::new();
    let mut passed = 0usize;

    run_case_group(&mut shell, CASES, &mut passed, &mut report, &flush).await;

    // Only the in-process builtin counts — on dev hosts a system python3
    // found via PATH would otherwise hijack these cases.
    let mut total = CASES.len();

    let (_c, py_probe_out) = run_case(&mut shell, "type python3 2>/dev/null").await;
    if py_probe_out.contains("builtin") {
        run_case_group(&mut shell, PY_CASES, &mut passed, &mut report, &flush).await;
        total += PY_CASES.len();
    } else {
        report.push_str("NOTE python3 not built in; python cases skipped\n");
    }

    let (_c, node_probe_out) = run_case(&mut shell, "type node 2>/dev/null").await;
    if node_probe_out.contains("builtin") {
        run_case_group(&mut shell, NODE_CASES, &mut passed, &mut report, &flush).await;
        total += NODE_CASES.len();
    } else {
        report.push_str("NOTE node not built in; node cases skipped\n");
    }

    report.push_str(&format!("=== {passed}/{total} passed ===\n"));
    flush(&report);
    report
}

/// Runs a group of cases, appending PASS/FAIL lines and counting passes,
/// flushing the running report after each case.
async fn run_case_group(
    shell: &mut brush_core::Shell,
    cases: &[Case],
    passed: &mut usize,
    report: &mut String,
    flush: &impl Fn(&str),
) {
    for case in cases {
        let (exit_code, output) = run_case(shell, case.script).await;
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
            *passed += 1;
            report.push_str(&format!("PASS {}\n", case.name));
        } else {
            report.push_str(&format!("FAIL {}: {}\n", case.name, failures.join("; ")));
        }
        flush(report);
    }
}

async fn run_case(shell: &mut brush_core::Shell, script: &str) -> (i32, String) {
    let (mut reader, writer) = std::io::pipe().expect("pipe");
    let out = OpenFile::from(writer);
    shell.open_files_mut().set_fd(1.into(), out.clone());
    shell.open_files_mut().set_fd(2.into(), out);

    let params = shell.default_exec_params();
    let source_info = brush_core::SourceInfo::from("selftest");
    let run = shell.run_string(script.to_string(), &source_info, &params);
    let result = tokio::time::timeout(std::time::Duration::from_secs(120), run).await;

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

// ---------------------------------------------------------------- flag audit

/// Reference flag sets, per command, taken from the GNU/upstream man pages.
///
/// This exists because a *partial* CLI is worse than a missing command: an
/// agent writes the flags every other Unix accepts, and a rejected one reads as
/// "I got the syntax wrong", so it retries variations that also fail. Today
/// that gap was only ever found by accident — `grep -iE` rejecting `-E` killed
/// a whole selftest run before anyone noticed grep had 9 flags. This audit
/// makes the gap a number that CI can watch instead.
///
/// A flag counts as supported when the command does not reject it as unknown.
/// It is deliberately a coverage signal, not a correctness test — behaviour is
/// pinned by the battery above.
const FLAG_REFERENCE: &[(&str, &[&str])] = &[
    // Taken from the GNU grep man page, NOT from what we happen to implement.
    // The earlier version of this list was written alongside a hand-written
    // grep, so it scored 36/36 while omitting the flags that grep had no
    // implementation for — a reference set derived from the implementation
    // cannot report a gap, which is the one thing this audit exists to do.
    (
        "grep",
        &[
            "-E",
            "-F",
            "-G",
            "-P",
            "-i",
            "-y",
            "-v",
            "-w",
            "-x",
            "-c",
            "-l",
            "-L",
            "-o",
            "-q",
            "-s",
            "-b",
            "-H",
            "-h",
            "-r",
            "-R",
            "-a",
            "-I",
            "-U",
            "-z",
            "-Z",
            "-T",
            "-m",
            "-A",
            "-B",
            "-C",
            "-e",
            "-f",
            "-d",
            "-D",
            "--include",
            "--exclude",
            "--exclude-dir",
            "--exclude-from",
            "--color",
            "--label",
            "--line-buffered",
            "--binary-files",
            "--group-separator",
            "--no-group-separator",
            "--no-ignore-case",
            "--null-data",
        ],
    ),
    (
        "rg",
        &[
            "-e",
            "-f",
            "-F",
            "-P",
            "-i",
            "-S",
            "-s",
            "-w",
            "-x",
            "-v",
            "-U",
            "-m",
            "-c",
            "-l",
            "-o",
            "-q",
            "-n",
            "-N",
            "-b",
            "-H",
            "-I",
            "-M",
            "-r",
            "-A",
            "-B",
            "-C",
            "-g",
            "-t",
            "-T",
            "-L",
            "-j",
            "-p",
            "-a",
            "--hidden",
            "--no-ignore",
            "--files",
            "--type-list",
            "--max-depth",
            "--column",
            "--heading",
            "--no-heading",
            "--stats",
            "--trim",
            "--null",
        ],
    ),
    (
        "sed",
        &[
            "-n",
            "-e",
            "-f",
            "-i",
            "-E",
            "-s",
            "-z",
            "-u",
            "--posix",
            "--debug",
            "--sandbox",
        ],
    ),
    (
        "find",
        &[
            "-name",
            "-iname",
            "-type",
            "-maxdepth",
            "-mindepth",
            "-size",
            "-mtime",
            "-newer",
            "-print",
            "-print0",
            "-delete",
            "-exec",
            "-empty",
            "-perm",
            "-user",
            "-path",
            "-prune",
            "-depth",
            "-follow",
            "-not",
            "-o",
            "-a",
        ],
    ),
    (
        "xargs",
        &[
            "-0", "-n", "-I", "-i", "-L", "-P", "-r", "-t", "-a", "-d", "-E", "-s", "-x",
        ],
    ),
    (
        "diff",
        &[
            "-u", "-c", "-e", "-y", "-q", "-s", "-t", "-U", "-C", "--normal", "--brief",
        ],
    ),
    (
        "cmp",
        &["-l", "-s", "-b", "-n", "-i", "--print-bytes", "--verbose"],
    ),
    (
        "tar",
        &[
            "-c",
            "-x",
            "-t",
            "-f",
            "-z",
            "-j",
            "-J",
            "-v",
            "-C",
            "--exclude",
            "-r",
            "-u",
        ],
    ),
    (
        "jq",
        &[
            "-r",
            "-c",
            "-n",
            "-e",
            "-s",
            "-j",
            "-a",
            "--arg",
            "--argjson",
            "--slurpfile",
            "--tab",
        ],
    ),
    ("egrep", &[]),
    ("fgrep", &[]),
    (
        "stat",
        &["-c", "--format", "-f", "--file-system", "-L", "-t"],
    ),
    ("zip", &["-r", "-q", "-9", "-j", "-d", "-u", "-x", "-@"]),
    ("unzip", &["-l", "-o", "-d", "-q", "-j", "-n", "-p", "-t"]),
    ("which", &["-a", "-s"]),
    ("tree", &["-a", "-d", "-L", "-f", "-i", "-P", "-I"]),
    ("gzip", &["-d", "-k", "-f", "-9", "-c", "-r", "-t"]),
    (
        "curl",
        &[
            "-X",
            "-H",
            "-d",
            "-o",
            "-O",
            "-L",
            "-s",
            "-i",
            "-I",
            "-u",
            "-A",
            "-f",
            "-k",
            "--json",
            "--data-raw",
            "--connect-timeout",
            "--max-time",
            "--retry",
            "--retry-delay",
            "--retry-all-errors",
        ],
    ),
    (
        "wget",
        &[
            "-O",
            "-q",
            "-c",
            "-P",
            "-nc",
            "-4",
            "-6",
            "-U",
            "-e",
            "--no-check-certificate",
            "--timeout",
            "--connect-timeout",
            "--max-redirect",
            "--header",
            "--waitretry",
            "--referer",
            "--user-agent",
            "--load-cookies",
            "--save-cookies",
            "--user",
            "--password",
            "--proxy-user",
            "--proxy-password",
            "--method",
            "--body-data",
            "--body-file",
            "--post-data",
            "--post-file",
            "--retry-connrefused",
            "--no-proxy",
            "--ignore-length",
            "--no-cache",
        ],
    ),
];

/// Markers a CLI emits when it does not recognise an option. Anything else —
/// a missing operand, a bad value, a nonexistent file — means the flag itself
/// was accepted.
const UNKNOWN_MARKERS: &[&str] = &[
    "unexpected argument",
    "unknown option",
    "unknown argument",
    "unrecognized option",
    "unsupported option",
    "invalid option",
    "unknown flag",
];

/// Probes every flag in [`FLAG_REFERENCE`] and reports per-command coverage.
pub fn run_flag_coverage(workdir: &std::path::Path) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(run_flag_coverage_async(workdir))
}

async fn run_flag_coverage_async(workdir: &std::path::Path) -> String {
    let scratch = workdir.join(format!("flagaudit_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    let mut shell = match crate::build_shell(std::collections::HashMap::new(), &scratch).await {
        Ok(s) => s,
        Err(e) => return format!("FATAL: shell init failed: {e}\n"),
    };

    let mut report = String::new();
    for (cmd, flags) in FLAG_REFERENCE {
        // Skip commands not built into this configuration rather than reporting
        // them as 0% — that would be a false alarm, and a false alarm is how a
        // real one gets ignored.
        let (_c, probe) = run_case(&mut shell, &format!("type {cmd} 2>/dev/null")).await;
        if !probe.contains("builtin") {
            report.push_str(&format!("SKIP {cmd} (not built in)\n"));
            continue;
        }
        let mut missing: Vec<&str> = Vec::new();
        for flag in *flags {
            // stdin from /dev/null so a flag that makes the command read input
            // cannot hang the audit.
            let script = format!("{cmd} {flag} </dev/null 2>&1 | head -5");
            let (_code, out) = run_case(&mut shell, &script).await;
            let lower = out.to_lowercase();
            if UNKNOWN_MARKERS.iter().any(|m| lower.contains(m)) {
                missing.push(flag);
            }
        }
        let have = flags.len() - missing.len();
        let pct = if flags.is_empty() {
            100
        } else {
            have * 100 / flags.len()
        };
        report.push_str(&format!("{cmd}: {have}/{} ({pct}%)", flags.len()));
        if missing.is_empty() {
            report.push('\n');
        } else {
            report.push_str(&format!("  MISSING {}\n", missing.join(" ")));
        }
    }
    report
}

#[cfg(test)]
mod command_matrix_tests {
    use super::*;

    fn contains_token(text: &str, token: &str) -> bool {
        text.match_indices(token).any(|(start, _)| {
            let before = text[..start].chars().next_back();
            let end = start + token.len();
            let after = text[end..].chars().next();
            let boundary = |c: Option<char>| {
                c.is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
            };
            boundary(before) && boundary(after)
        })
    }

    #[test]
    fn every_registered_command_is_named_by_a_functional_case() {
        let mut all_cases: Vec<&Case> = CASES.iter().collect();
        all_cases.extend(PY_CASES);
        all_cases.extend(NODE_CASES);
        let missing: Vec<String> = crate::command_inventory()
            .into_iter()
            .filter(|command| {
                !all_cases.iter().any(|case| {
                    contains_token(case.script, &command.name)
                        || contains_token(case.name, &command.name)
                })
            })
            .map(|command| command.name)
            .collect();
        assert!(
            missing.is_empty(),
            "registered commands without a functional test case: {}",
            missing.join(", ")
        );
    }

    #[test]
    fn command_inventory_is_unique_and_sorted() {
        let inventory = crate::command_inventory();
        assert!(inventory.windows(2).all(|pair| pair[0].name < pair[1].name));
    }
}
