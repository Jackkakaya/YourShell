# YourShell Tool System Audit

Code snapshot: `a2cdddae7785ae75e19b866493fa5e8573fa11e6` plus this audit.

The source of truth is `core/src/lib.rs::build_shell`, Brush's
`default_builtins(BuiltinSet::BashMode)`, and
`brush_coreutils_builtins::bundled_commands()`. The current iOS build exposes
178 unique command names:

- 61 Brush Bash builtins
- 74 uutils/coreutils commands
- 43 explicitly registered project/upstream/Host commands

Legend:

- **OK**: the integration shape and useful iOS behaviour match the intended
  contract.
- **PARTIAL**: useful and intentional, but an important CLI, lifecycle,
  cancellation, concurrency, or iOS semantic is missing.
- **NO**: registered but unimplemented, unsafe, or fundamentally misleading on
  iOS.

## Shared execution models

| Model | How it works | Assessment |
|---|---|---|
| Brush builtin | Direct `ExecutionContext`; reads and mutates the current Shell | Correct model for `cd`, `export`, `read`, aliases, functions, and other Shell state |
| Process Host | `command_host::dispatch` snapshots Session argv/cwd/env/fds, takes one global lock, swaps process state, calls upstream, then restores it | Correct thin argv integration, but cross-Session commands serialize; piped stdin is fully buffered; blocking work is not truly cancellable |
| Session-native Rust | Adapter consumes Session fds/cwd/env directly | Preferred model for concurrent long-running commands |
| iOS Host | Rust command calls an app-installed UIKit/Vision callback | Correct boundary for clipboard, URL opening, OCR |
| Resident runtime | Node/Python runtime stays alive; adapter sends each invocation and Session state | Correct iOS lifecycle direction, with runtime-specific package/process limits |

## Brush Bash builtins — 61

| Command | Integration | Result | Notes |
|---|---|---|---|
| `.` | Brush `dot` builtin | OK | Sources into the current Shell; state intentionally persists |
| `:` | Brush `colon` builtin | OK | POSIX no-op |
| `[` | Brush `test` builtin | OK | Bash/POSIX expression semantics |
| `alias` | Brush builtin | OK | Session-local alias table |
| `bg` | Brush job builtin | PARTIAL | Only Brush-managed jobs; no normal child-process job control on iOS |
| `bind` | Brush builtin | PARTIAL | Brush line-editor bindings are not the SwiftTerm UI's primary key-input layer |
| `break` | Brush special builtin | OK | Mutates current control flow |
| `builtin` | Brush raw-argument builtin | OK | Dispatches current Shell builtins |
| `caller` | Brush builtin | OK | Shell call-stack metadata |
| `cd` | Brush builtin | OK | Correctly mutates only the current Session cwd |
| `command` | Brush builtin | OK | Bypasses functions/aliases according to Shell lookup |
| `compgen` | Brush completion builtin | OK | Uses current Shell symbols and command table |
| `complete` | Brush completion builtin | OK | Session completion definitions |
| `compopt` | Brush completion builtin | OK | Session completion options |
| `continue` | Brush special builtin | OK | Mutates current control flow |
| `declare` | Brush declaration builtin | OK | Current Shell variables/attributes |
| `dirs` | Brush directory-stack builtin | OK | Session-local directory stack |
| `disown` | Brush `UnimplementedCommand` | NO | Name is registered but implementation is explicitly absent |
| `echo` | Brush builtin | OK | Kept over uutils so Shell semantics and no global lock win |
| `enable` | Brush builtin | OK | Session builtin enable/disable state |
| `eval` | Brush special builtin | OK | Evaluates in current Shell |
| `exec` | Brush Unix builtin | NO | No-argument form works, but command form tries OS `exec`; replacing an iOS app process is not a supported command model |
| `exit` | Brush special builtin | OK | Exits Shell/Session, not the host app process |
| `export` | Brush declaration builtin | OK | Correctly mutates current Session environment |
| `false` | Brush builtin | OK | Correct status without Host overhead |
| `fc` | Brush history builtin | PARTIAL | Shell history works; external editor-dependent forms need iOS-specific validation |
| `fg` | Brush job builtin | PARTIAL | Only Brush-managed jobs; normal process groups/TTY ownership do not exist |
| `getopts` | Brush builtin | OK | Mutates current Shell option variables |
| `hash` | Brush builtin | OK | Session command-lookup cache |
| `help` | Brush builtin | OK | Describes registered builtins |
| `history` | Brush builtin | OK | Persistent for the lifetime of the Session |
| `jobs` | Brush job builtin | PARTIAL | Reports Brush jobs, not general Unix child processes |
| `kill` | Brush Unix builtin | PARTIAL | Brush jobs/signals only; arbitrary PID signalling is inappropriate and potentially dangerous in an app |
| `let` | Brush builtin | OK | Current Shell arithmetic/variables |
| `local` | Brush declaration builtin | OK | Function-local variables |
| `logout` | Brush `UnimplementedCommand` | NO | Name is registered but implementation is explicitly absent |
| `mapfile` | Brush builtin | OK | Reads through Session stdin and updates Shell arrays |
| `popd` | Brush directory-stack builtin | OK | Session-local |
| `printf` | Brush builtin | OK | Kept over uutils for Shell semantics |
| `pushd` | Brush directory-stack builtin | OK | Session-local |
| `pwd` | Brush builtin | OK | Reads Session cwd, not process cwd |
| `read` | Brush builtin | OK | Reads Session stdin and mutates current variables |
| `readarray` | Brush `mapfile` alias | OK | Same implementation as `mapfile` |
| `readonly` | Brush declaration builtin | OK | Current Shell variables |
| `return` | Brush special builtin | OK | Current function/source control flow |
| `set` | Brush special builtin | OK | Current Shell options and positional arguments |
| `shift` | Brush special builtin | OK | Current positional arguments |
| `shopt` | Brush builtin | OK | Current Shell options |
| `source` | Brush `dot` alias | OK | Sources into current Shell |
| `suspend` | Brush Unix signal builtin | NO | Sends `SIGTSTP` to the app process; wrong lifecycle operation for iOS |
| `test` | Brush builtin | OK | Kept over uutils for Shell semantics |
| `times` | Brush builtin | PARTIAL | Process accounting is of limited meaning without child processes |
| `trap` | Brush special builtin | PARTIAL | Shell traps work; Unix signal coverage is constrained by the iOS app host |
| `true` | Brush builtin | OK | Correct status without Host overhead |
| `type` | Brush builtin | OK | Uses actual Shell functions/builtins/PATH lookup |
| `typeset` | Brush `declare` alias | OK | Current Shell variables |
| `ulimit` | Brush Unix builtin | PARTIAL | Resource limits are process-global and can affect the whole app/all Sessions |
| `umask` | Brush Unix builtin | PARTIAL | Functional, but process-global rather than Session-local |
| `unalias` | Brush builtin | OK | Session-local alias table |
| `unset` | Brush special builtin | OK | Current Shell variables/functions |
| `wait` | Brush job builtin | PARTIAL | Useful for Brush jobs; no general child-process waiting |

## uutils/coreutils — 74

Every row below uses the same thin integration:

`command name -> bundled_commands registry -> upstream uu_*::uumain(argv) -> command_host`

YourShell does not define or parse their flags. “OK” means the finite,
non-privileged form is suitable on iOS. Every row still inherits the Process
Host's global serialization, buffered-pipeline, and best-effort-cancellation
limitations described above.

| Command | Result | iOS/compatibility note |
|---|---|---|
| `arch` | OK | Reports the app process architecture |
| `b2sum` | OK | Upstream hashing CLI |
| `base32` | OK | Upstream encoder/decoder |
| `base64` | OK | Upstream encoder/decoder |
| `basename` | OK | Pure path/text operation |
| `basenc` | OK | Upstream multi-base encoder |
| `cat` | PARTIAL | File/pipeline forms work; interactive wait holds the global Process Host lock |
| `cksum` | OK | Upstream checksum CLI |
| `comm` | OK | Finite file/stdin operation |
| `cp` | OK | Patched uutils implementation; sandbox filesystem only |
| `csplit` | OK | Finite file operation |
| `cut` | OK | Finite file/stdin operation |
| `date` | PARTIAL | Formatting works; setting system time is unavailable on iOS |
| `dd` | PARTIAL | Core copying works; long/blocking copies cannot be cooperatively cancelled |
| `df` | OK | Reports filesystems visible to the sandbox |
| `dir` | OK | uutils `ls` presentation alias |
| `dircolors` | OK | Produces color configuration; terminal consumption is caller-controlled |
| `dirname` | OK | Pure path/text operation |
| `du` | OK | Sandbox filesystem only |
| `env` | PARTIAL | Printing/temporary environment works; launching an external executable is not an iOS execution model |
| `expand` | OK | Finite text transformation |
| `expr` | OK | Pure expression operation |
| `factor` | OK | Pure numeric operation |
| `fmt` | OK | Finite text transformation |
| `fold` | OK | Finite text transformation |
| `head` | OK | Finite forms work and are tested |
| `hostname` | PARTIAL | Reading works; changing device hostname is unavailable |
| `join` | OK | Finite file operation |
| `link` | OK | Sandbox filesystem only |
| `ln` | OK | Sandbox filesystem only |
| `ls` | OK | Common forms tested; sandbox filesystem only |
| `md5sum` | OK | Upstream hashing CLI |
| `mkdir` | OK | Sandbox filesystem only |
| `mktemp` | OK | Uses app-accessible temporary/filesystem locations |
| `mv` | OK | Sandbox filesystem only |
| `nl` | OK | Finite text transformation |
| `nproc` | OK | Reports CPUs visible to the app process |
| `numfmt` | OK | Pure text/numeric operation |
| `od` | OK | Finite binary/text operation |
| `paste` | OK | Finite file/stdin operation |
| `pr` | OK | Finite text formatting |
| `printenv` | OK | Receives the invoking Session's exported environment snapshot |
| `ptx` | OK | Finite text transformation |
| `readlink` | OK | Sandbox filesystem only |
| `realpath` | OK | Resolves paths visible to the sandbox |
| `rm` | OK | Sandbox filesystem only |
| `rmdir` | OK | Sandbox filesystem only |
| `seq` | OK | Finite numeric generation |
| `sha1sum` | OK | Upstream hashing CLI |
| `sha224sum` | OK | Upstream hashing CLI |
| `sha256sum` | OK | Upstream hashing CLI |
| `sha384sum` | OK | Upstream hashing CLI |
| `sha512sum` | OK | Upstream hashing CLI |
| `shred` | PARTIAL | CLI runs, but secure erasure cannot be promised on APFS/flash storage |
| `shuf` | OK | Finite text randomization |
| `sleep` | PARTIAL | Sleeps correctly but holds the Process Host lock and is not truly cancellable |
| `sort` | OK | Common key/numeric/unique forms tested |
| `split` | OK | Finite file operation |
| `sum` | OK | Upstream checksum CLI |
| `sync` | PARTIAL | Process/system sync semantics are weak and global inside an iOS sandbox |
| `tac` | OK | Finite file/stdin operation |
| `tail` | PARTIAL | Finite forms work; `-f` is long-running, globally serializing, and not cooperatively cancellable |
| `tee` | PARTIAL | Output is correct, but piped stdin is fully buffered before execution rather than streamed |
| `touch` | OK | Sandbox filesystem only |
| `tr` | OK | Finite text transformation |
| `truncate` | OK | Sandbox filesystem only |
| `tsort` | OK | Finite text/graph operation |
| `uname` | OK | Reports the iOS/Darwin app host |
| `unexpand` | OK | Finite text transformation |
| `uniq` | OK | Finite text transformation |
| `unlink` | OK | Sandbox filesystem only |
| `vdir` | OK | uutils `ls` presentation alias |
| `wc` | OK | Finite file/stdin operation |
| `whoami` | OK | Reports the app process user identity |

Not exposed intentionally:

- `yes`: skipped because an infinite Process Host command cannot currently be
  interrupted safely.
- `more`: skipped because its TTY/pager model is not integrated.
- uutils versions of `echo`, `printf`, `pwd`, `test`, `true`, and `false`:
  Brush versions intentionally win.

## Explicit project/upstream/Host commands — 43

| Command | Integration | Result | Notes |
|---|---|---|---|
| `awk` | Vendored one-true-awk C CLI through Process Host | PARTIAL | Real awk language; `system()` and command pipes are disabled because iOS has no fork/exec |
| `clear` | Small direct Brush command | OK | Emits ANSI clear/home/scrollback sequences |
| `cmp` | Patched uutils/diffutils argv entry through Process Host | OK | Upstream parser/implementation; exits patched to return status |
| `curl` | Official curl 8.1.2 C CLI through Process Host | PARTIAL | Real curl argv/parser and enabled protocols; old upstream version, build-feature subset, global lock, weak cancellation |
| `diff` | Patched uutils/diffutils argv entry through Process Host | OK | Upstream formats/parser; exits and argv entry patched for embedding |
| `edit` | Local Rust terminal editor | PARTIAL | Useful local editor, not an upstream `edit` compatibility target |
| `egrep` | uutils/grep alias through Process Host | OK | Real grep engine with ERE mode injected |
| `fgrep` | uutils/grep alias through Process Host | OK | Real grep engine with fixed-string mode injected |
| `find` | Vendored uutils/findutils through Process Host | PARTIAL | Real predicates; `-exec` is patched to an in-process subshell instead of a child process |
| `git` | Handled porcelain over `git2-rs`/libgit2 | PARTIAL | Strong common workflow including pull/rebase; not the full Git CLI, protocol/helper/hook/filter/LFS/submodule/worktree surface |
| `grep` | uutils/grep `uumain(argv)` through Process Host | OK | Real GNU-style parser and regex semantics |
| `gunzip` | BSD gzip C frontend through Process Host | OK | Upstream CLI entry; finite archive operations |
| `gzip` | BSD gzip C frontend through Process Host | OK | Upstream CLI entry; finite archive operations |
| `jq` | Vendored jq 1.8.2 C CLI through Process Host | OK | Real jq language; process-exit/stdout-close paths patched for embedding |
| `mosh` | Project pure-Rust client, SSH bootstrap + UDP/OCB/SSP | PARTIAL | Architecture is iOS-native; real mosh-server interoperability and lifecycle/network-switch matrix remain insufficiently proven |
| `nano` | Alias to local Rust terminal editor | PARTIAL | Not GNU nano-compatible despite the command name |
| `node` | Resident nodejs-mobile runtime over authenticated loopback | PARTIAL | JS/files/modules work; no child processes or native addons; lifecycle/cancellation still need stronger testing |
| `npm` | npm CLI executed inside resident Node | PARTIAL | Pure-JS packages work; install lifecycle scripts are disabled and native addons cannot build |
| `npx` | npx CLI executed inside resident Node | PARTIAL | Same Node/iOS package restrictions as `npm` |
| `ocr` | Vision-framework iOS Host | OK | Correct Host boundary; only present in builds with `vision` |
| `open` | UIKit iOS Host callback | PARTIAL | Opens URLs/files supported by Host; not macOS `/usr/bin/open`'s complete CLI |
| `openurl` | Alias to `open` Host callback | OK | Explicit URL-oriented alias |
| `pbcopy` | UIKit clipboard Host callback | OK | Reads Session stdin, writes system pasteboard |
| `pbpaste` | UIKit clipboard Host callback | OK | Reads system pasteboard, writes Session stdout |
| `pip` | Embedded CPython driver | PARTIAL | Pure Python and bundled iOS wheels work; source/native builds and subprocess-dependent packages cannot |
| `pip3` | Alias to embedded CPython pip driver | PARTIAL | Same constraints as `pip` |
| `python` | Embedded resident CPython | PARTIAL | Scripts/stdin/cwd/env work; no fork/subprocess, and native imports require iOS-built extensions |
| `python3` | Embedded resident CPython | PARTIAL | Same implementation as `python`; simulator scientific runtime suite passes 9/9 |
| `rg` | Official ripgrep CLI core through Process Host | OK | Upstream argv parser/search semantics; finite searches |
| `scp` | russh + SFTP, Session-native | PARTIAL | Concurrent and iOS-safe; implements useful transfer subset, not the complete OpenSSH scp CLI/protocol surface |
| `sed` | uutils/sed `uumain(argv)` through Process Host | OK | Real parser including `-i`, `-E`, `-n`, `-f`; one upstream `-i` spelling normalization |
| `sftp` | russh-sftp line REPL, Session-native | PARTIAL | Useful command subset; not the complete OpenSSH interactive command/option surface |
| `sqlite3` | Official SQLite shell C frontend through Process Host | OK | Real dot commands, formats, parser, and SQL engine; process-exit paths contained |
| `ssh` | russh interactive client, Session-native | PARTIAL | PTY/auth/known-host flow exists; not the full OpenSSH config, forwarding, agent, ProxyJump, multiplexing surface |
| `stat` | Official uutils/stat `uumain(argv)` through Process Host | OK | Separate because Brush's curated coreutils bundle omitted it |
| `tar` | Official libarchive bsdtar C frontend through Process Host | OK | Upstream parsing/archive implementation; supported formats depend on linked libarchive build |
| `tree` | Local Rust/Clap implementation | PARTIAL | Seven high-value flags; not full upstream tree formatting, metadata, color, sorting, symlink options |
| `unzip` | Official libarchive bsdunzip frontend through Process Host | OK | Upstream parser/archive implementation |
| `vi` | Alias to local Rust terminal editor | PARTIAL | Vi-like editor, not complete Vim/vi compatibility |
| `wget` | Handwritten Wget-to-curl argv translator | PARTIAL | Common HTTP download flags; not GNU Wget recursion/mirroring/full option semantics |
| `which` | Local Brush-aware Clap command | PARTIAL | Correct builtin/function/PATH lookup and `-a/-s`; not every platform-specific `which` option |
| `xargs` | Vendored uutils/findutils through Process Host | PARTIAL | Real parser; command execution is redirected to an in-process subshell, and input is buffered |
| `zip` | Local Clap CLI over Rust `zip` crate | PARTIAL | Common create/update/delete/store/exclude/stdin forms; no encryption, split archives, or complete Info-ZIP surface |

## Test evidence

- Native command battery: **337/337 passed**.
- Flag reference gate: all listed commands passed; `wget` reports 31/32 only
  because the reference contains bare `-n`, while useful Wget flags are
  `-nc`, `-nv`, and `-nH`. Do not add a fake `-n`.
- Git compatibility suite passes local state, clone, stash, merge,
  cherry-pick, revert, restore, clean, amend, branch/remote/config, push,
  force-with-lease, pull merge/ff-only, and pull/rebase
  continue/abort/autostash/config flows.
- Python iOS simulator suite: **9/9 passed** for NumPy, SciPy, Pandas, Pillow,
  Matplotlib, Pyparsing, contourpy, kiwisolver, and FontTools.
- The native battery does not link Python or Node, so it explicitly skips
  those runtime cases.
- The current “safe concurrency” test only covers four concurrent
  `git --version` invocations. It is not proof of general multi-Session
  concurrency.

## Verdict

The command-selection and argv-forwarding direction now mostly matches the
intended architecture. It is not yet accurate to say the tool system as a
whole is finished:

1. Remove or replace misleading/unsafe registered builtins: `disown`,
   `logout`, command-form `exec`, and `suspend`.
2. Split Process Host isolation by Session or replace it with injected
   cwd/env/stdio entry points. Today one `cat`, `sleep`, `tail -f`, curl
   transfer, or Python invocation can serialize unrelated Sessions.
3. Wire cooked-mode Ctrl-C to `ashell_cancel`, and do not report a command
   finished while its blocking worker is still running against global state.
4. Decide compatibility contracts for all `PARTIAL` branded commands. The
   most misleading names today are `nano`, `vi`, `wget`, `git`, `ssh`,
   `sftp`, `scp`, and `mosh`.
5. Add permanent simulator/device tests for Node, SSH/SFTP/SCP, mosh-server
   interoperability, cancellation, background/resume, and multi-Session
   overlap.
