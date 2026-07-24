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
    case!("uname", "uname", Has("Darwin")),
    case!("clear", "clear -x | wc -c", Has("7")),
    case!("clear-scrollback", "clear | od -c | head -1", Has("033")),
    // ---- ext commands (crate-backed) ----
    case!("which-builtin", "which cd", Has("builtin")),
    case!("which-cmd", "which grep 2>&1 | head -1", Has("grep")),
    case!("find-name", "mkdir -p ft/sub; touch ft/a.txt ft/sub/b.txt ft/c.log; find ft --name '*.txt' | sort | tr '\\n' ' '", Has("a.txt"), Has("b.txt"), Not("c.log")),
    case!("find-type-d", "mkdir -p fd/x fd/y; find fd --type d | wc -l", Has("3")),
    case!("find-maxdepth", "mkdir -p md/1/2/3; find md --type d --maxdepth 1 | wc -l", Has("2")),
    case!("tree-basic", "mkdir -p tr/a tr/b; touch tr/a/f.txt; tree tr", Has("a"), Has("f.txt"), Has("directories")),
    case!("diff-changed", "printf 'a\\nb\\nc\\n' > d1.txt; printf 'a\\nX\\nc\\n' > d2.txt; diff d1.txt d2.txt; echo rc=$?", Has("-b"), Has("+X"), Has("rc=1")),
    case!("diff-same", "printf 'a\\nb\\n' > s1.txt; cp s1.txt s2.txt; diff s1.txt s2.txt; echo rc=$?", Eq("rc=0")),
    case!("gzip-roundtrip", "echo 'compress me please' > gz.txt; gzip gz.txt; [ -f gz.txt.gz ] && gunzip gz.txt.gz && cat gz.txt", Eq("compress me please")),
    case!("gzip-stdin-pipe", "echo hello | gzip -c | gunzip -c", Eq("hello")),
    case!("sed-substitute", "printf 'foo bar\\nfoo baz\\n' | sed 's/foo/XXX/'", Eq("XXX bar\nXXX baz")),
    case!("sed-global", "echo 'a a a' | sed 's/a/b/g'", Eq("b b b")),
    case!("sed-e-multi", "echo hello | sed -e 's/h/H/' -e 's/o/O/'", Eq("HellO")),
    case!("tar-roundtrip", "mkdir -p td/x; echo content > td/x/f.txt; tar -czf t.tgz td; rm -rf td; tar -xzf t.tgz; cat td/x/f.txt", Eq("content")),
    case!("tar-list", "mkdir -p tl; echo a > tl/a.txt; tar -cf tl.tar tl; tar -tf tl.tar | grep -c txt", Has("1")),
    case!("zip-unzip-roundtrip", "mkdir -p zt; echo zipped > zt/z.txt; zip -r z.zip zt > /dev/null; rm -rf zt; unzip z.zip > /dev/null; cat zt/z.txt", Eq("zipped")),
    case!("unzip-list", "echo hi > u.txt; zip u.zip u.txt > /dev/null; unzip -l u.zip | grep -c u.txt", Has("1")),
    case!("sqlite-memory", "sqlite3 :memory: 'create table t(a,b); insert into t values(1,2),(3,4); select sum(a),sum(b) from t'", Eq("4|6")),
    case!("sqlite-file", "sqlite3 test.db 'create table u(name text)'; sqlite3 test.db \"insert into u values('alice')\"; sqlite3 test.db 'select name from u'", Eq("alice")),
    case!("sqlite-stdin", "echo 'select 6*7' | sqlite3 :memory:", Eq("42")),
    case!("sqlite-header", "sqlite3 --header :memory: 'select 1 as x, 2 as y'", Eq("x|y\n1|2")),
    case!("jq-field", "echo '{\"name\":\"alice\",\"age\":30}' | jq '.name'", Eq("\"alice\"")),
    case!("jq-raw", "echo '{\"name\":\"alice\"}' | jq -r '.name'", Eq("alice")),
    case!("jq-array-map", "echo '[1,2,3]' | jq 'map(. * 2)'", Has("2"), Has("4"), Has("6")),
    case!("jq-pipe", "echo '{\"a\":{\"b\":42}}' | jq '.a.b'", Eq("42")),
    case!("jq-keys", "echo '{\"x\":1,\"y\":2}' | jq -r 'keys[]'", Eq("x\ny")),
    case!("jq-select", "echo '[{\"n\":1},{\"n\":5}]' | jq '.[] | select(.n > 3) | .n'", Eq("5")),
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
    case!("whoami-nproc-hostname", "whoami > /dev/null && nproc > /dev/null && hostname > /dev/null; echo id-rc=$?", Eq("id-rc=0")),
    case!("sleep-cmd", "sleep 0; echo slept=$?", Eq("slept=0")),
    case!("od", "printf 'A' | od -An -c", Has("A")),
    case!("fold", "echo abcdef | fold -w 3", Eq("abc\ndef")),
    case!("split-cmd", "printf '1\\n2\\n3\\n4\\n' > sp.txt; split -l 2 sp.txt spx_; cat spx_aa", Eq("1\n2")),
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
