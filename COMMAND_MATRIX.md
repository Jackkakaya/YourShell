# YourShell 171 个命令接入矩阵

目标模式指：

```text
Brush argv/cwd/env/fd
        ↓
统一 Adapter / Command Host
        ↓
上游可调用 CLI 入口（main/uumain/run）
        ↓
exit code
```

判断：

- **已是目标模式**：已经复用上游 argv 解析和命令实现。
- **应保留 Brush**：必须读写当前 Shell 状态，不能改成进程型 CLI。
- **可保留 Brush**：简单 Shell builtin，Brush 实现比外部 CLI 更自然。
- **应寻找上游 CLI**：当前在本地重新定义成熟 CLI，应优先改造。
- **保留专用 Host**：语言运行时、系统 Framework 或交互协议，不适合普通 `uumain`，但可以统一外层接口。

## 1. Brush Bash builtins（61）

| # | 命令 | 当前实现 | 当前入口/解析 | 目标判断 |
|---:|---|---|---|---|
| 1 | `.` | Brush DotCommand | Brush builtin；解析 source 文件参数 | 应保留 Brush：修改当前 Shell |
| 2 | `:` | Brush ColonCommand | Brush simple builtin | 可保留 Brush |
| 3 | `[` | Brush TestCommand | Brush builtin；解析 test 表达式 | 可保留 Brush |
| 4 | `alias` | Brush AliasCommand | Brush builtin | 应保留 Brush：修改 alias 表 |
| 5 | `bg` | Brush BgCommand | Brush builtin | 应保留 Brush：操作 Shell job |
| 6 | `bind` | Brush BindCommand | Brush builtin | 应保留 Brush：修改输入绑定 |
| 7 | `break` | Brush BreakCommand | Brush special builtin | 应保留 Brush：控制 Shell AST |
| 8 | `builtin` | Brush BuiltinCommand | Brush raw-argument builtin | 应保留 Brush：调用 builtin 注册表 |
| 9 | `caller` | Brush CallerCommand | Brush builtin | 应保留 Brush：读取调用栈 |
| 10 | `cd` | Brush CdCommand | Brush builtin | 应保留 Brush：修改当前 cwd |
| 11 | `command` | Brush CommandCommand | Brush builtin | 应保留 Brush：影响命令解析 |
| 12 | `compgen` | Brush CompGenCommand | Brush builtin | 应保留 Brush：读取补全状态 |
| 13 | `complete` | Brush CompleteCommand | Brush builtin | 应保留 Brush：修改补全规则 |
| 14 | `compopt` | Brush CompOptCommand | Brush builtin | 应保留 Brush：修改补全选项 |
| 15 | `continue` | Brush ContinueCommand | Brush special builtin | 应保留 Brush：控制 Shell AST |
| 16 | `declare` | Brush DeclareCommand | Brush declaration builtin | 应保留 Brush：修改变量 |
| 17 | `dirs` | Brush DirsCommand | Brush builtin | 应保留 Brush：读取目录栈 |
| 18 | `disown` | Brush UnimplementedCommand | Brush builtin；当前未实现 | 保留为 Brush 或移除，不能用普通 CLI 替代 |
| 19 | `echo` | Brush EchoCommand | Brush builtin | 可保留 Brush；已主动覆盖 uutils echo |
| 20 | `enable` | Brush EnableCommand | Brush builtin | 应保留 Brush：修改 builtin 状态 |
| 21 | `eval` | Brush EvalCommand | Brush special builtin | 应保留 Brush：在当前 Shell 执行源码 |
| 22 | `exec` | Brush ExecCommand | Brush special builtin | 应保留 Brush：属于 Shell 执行语义 |
| 23 | `exit` | Brush ExitCommand | Brush special builtin | 应保留 Brush：控制 Shell 生命周期 |
| 24 | `export` | Brush ExportCommand | Brush declaration/special builtin | 应保留 Brush：修改导出环境 |
| 25 | `false` | Brush FalseCommand | Brush simple builtin | 可保留 Brush；已主动覆盖 uutils false |
| 26 | `fc` | Brush FcCommand | Brush builtin | 应保留 Brush：操作历史 |
| 27 | `fg` | Brush FgCommand | Brush builtin | 应保留 Brush：操作 Shell job |
| 28 | `getopts` | Brush GetOptsCommand | Brush builtin | 应保留 Brush：写入 Shell 变量 |
| 29 | `hash` | Brush HashCommand | Brush builtin | 应保留 Brush：修改命令缓存 |
| 30 | `help` | Brush HelpCommand | Brush builtin | 应保留 Brush：读取 builtin 元数据 |
| 31 | `history` | Brush HistoryCommand | Brush builtin | 应保留 Brush：操作历史 |
| 32 | `jobs` | Brush JobsCommand | Brush builtin | 应保留 Brush：读取 Shell job |
| 33 | `kill` | Brush KillCommand | Brush builtin | 可保留 Brush；iOS job 能力受限 |
| 34 | `let` | Brush LetCommand | Brush builtin | 应保留 Brush：修改 Shell 变量 |
| 35 | `local` | Brush DeclareCommand | Brush declaration builtin | 应保留 Brush：修改函数局部变量 |
| 36 | `logout` | Brush UnimplementedCommand | Brush builtin；当前未实现 | 保留为 Brush 或移除 |
| 37 | `mapfile` | Brush MapFileCommand | Brush builtin | 应保留 Brush：写入数组变量 |
| 38 | `popd` | Brush PopdCommand | Brush builtin | 应保留 Brush：修改 cwd/目录栈 |
| 39 | `printf` | Brush PrintfCommand | Brush builtin | 可保留 Brush；已主动覆盖 uutils printf |
| 40 | `pushd` | Brush PushdCommand | Brush builtin | 应保留 Brush：修改 cwd/目录栈 |
| 41 | `pwd` | Brush PwdCommand | Brush builtin | 可保留 Brush；读取会话 cwd |
| 42 | `read` | Brush ReadCommand | Brush builtin | 应保留 Brush：将 stdin 写入变量 |
| 43 | `readarray` | Brush MapFileCommand | Brush builtin | 应保留 Brush：写入数组变量 |
| 44 | `readonly` | Brush DeclareCommand | Brush declaration/special builtin | 应保留 Brush：修改变量属性 |
| 45 | `return` | Brush ReturnCommand | Brush special builtin | 应保留 Brush：控制函数/source AST |
| 46 | `set` | Brush SetCommand | Brush special builtin | 应保留 Brush：修改 Shell options/参数 |
| 47 | `shift` | Brush ShiftCommand | Brush special builtin | 应保留 Brush：修改位置参数 |
| 48 | `shopt` | Brush ShoptCommand | Brush builtin | 应保留 Brush：修改 Shell options |
| 49 | `source` | Brush DotCommand | Brush special builtin | 应保留 Brush：在当前 Shell 执行源码 |
| 50 | `suspend` | Brush SuspendCommand | Brush builtin | 应保留 Brush；iOS 能力受限 |
| 51 | `test` | Brush TestCommand | Brush builtin | 可保留 Brush；已主动覆盖 uutils test |
| 52 | `times` | Brush TimesCommand | Brush builtin | 应保留 Brush：读取 Shell 进程统计 |
| 53 | `trap` | Brush TrapCommand | Brush special builtin | 应保留 Brush：修改 Shell trap |
| 54 | `true` | Brush TrueCommand | Brush simple builtin | 可保留 Brush；已主动覆盖 uutils true |
| 55 | `type` | Brush TypeCommand | Brush builtin | 应保留 Brush：查询函数/alias/builtin |
| 56 | `typeset` | Brush DeclareCommand | Brush declaration builtin | 应保留 Brush：修改变量 |
| 57 | `ulimit` | Brush ULimitCommand | Brush builtin | 可保留 Brush；操作宿主限制需谨慎 |
| 58 | `umask` | Brush UmaskCommand | Brush builtin | 应保留 Brush：影响后续文件创建 |
| 59 | `unalias` | Brush UnaliasCommand | Brush builtin | 应保留 Brush：修改 alias 表 |
| 60 | `unset` | Brush UnsetCommand | Brush special builtin | 应保留 Brush：修改变量/函数 |
| 61 | `wait` | Brush WaitCommand | Brush builtin | 应保留 Brush：等待 Shell job |

## 2. uutils/coreutils（74）

这 74 个命令共享 `core/src/uutils_adapter.rs`。命令名从
`brush_coreutils_builtins::bundled_commands()` 取得，按名称找到函数，
再经 `command_host` 调用上游 `uumain(argv)`。

| # | 命令 | 当前实现/入口 | flags 解析 | 目标判断 |
|---:|---|---|---|---|
| 62 | `arch` | `uu_arch::uumain` | uutils | 已是目标模式 |
| 63 | `b2sum` | `uu_b2sum::uumain` | uutils | 已是目标模式 |
| 64 | `base32` | `uu_base32::uumain` | uutils | 已是目标模式 |
| 65 | `base64` | `uu_base64::uumain` | uutils | 已是目标模式 |
| 66 | `basename` | `uu_basename::uumain` | uutils | 已是目标模式 |
| 67 | `basenc` | `uu_basenc::uumain` | uutils | 已是目标模式 |
| 68 | `cat` | `uu_cat::uumain` | uutils | 已是目标模式 |
| 69 | `cksum` | `uu_cksum::uumain` | uutils | 已是目标模式 |
| 70 | `comm` | `uu_comm::uumain` | uutils | 已是目标模式 |
| 71 | `cp` | patched `uu_cp::uumain` | uutils | 已是目标模式 |
| 72 | `csplit` | `uu_csplit::uumain` | uutils | 已是目标模式 |
| 73 | `cut` | `uu_cut::uumain` | uutils | 已是目标模式 |
| 74 | `date` | patched `uu_date::uumain` | uutils | 已是目标模式 |
| 75 | `dd` | `uu_dd::uumain` | uutils | 已是目标模式 |
| 76 | `df` | `uu_df::uumain` | uutils | 已是目标模式 |
| 77 | `dir` | `uu_dir::uumain` | uutils | 已是目标模式 |
| 78 | `dircolors` | `uu_dircolors::uumain` | uutils | 已是目标模式 |
| 79 | `dirname` | `uu_dirname::uumain` | uutils | 已是目标模式 |
| 80 | `du` | `uu_du::uumain` | uutils | 已是目标模式 |
| 81 | `env` | `uu_env::uumain` | uutils | 已是目标模式 |
| 82 | `expand` | `uu_expand::uumain` | uutils | 已是目标模式 |
| 83 | `expr` | `uu_expr::uumain` | uutils | 已是目标模式 |
| 84 | `factor` | `uu_factor::uumain` | uutils | 已是目标模式 |
| 85 | `fmt` | `uu_fmt::uumain` | uutils | 已是目标模式 |
| 86 | `fold` | `uu_fold::uumain` | uutils | 已是目标模式 |
| 87 | `head` | `uu_head::uumain` | uutils | 已是目标模式 |
| 88 | `hostname` | `uu_hostname::uumain` | uutils | 已是目标模式 |
| 89 | `join` | `uu_join::uumain` | uutils | 已是目标模式 |
| 90 | `link` | `uu_link::uumain` | uutils | 已是目标模式 |
| 91 | `ln` | `uu_ln::uumain` | uutils | 已是目标模式 |
| 92 | `ls` | `uu_ls::uumain` | uutils | 已是目标模式 |
| 93 | `md5sum` | `uu_md5sum::uumain` | uutils | 已是目标模式 |
| 94 | `mkdir` | `uu_mkdir::uumain` | uutils | 已是目标模式 |
| 95 | `mktemp` | `uu_mktemp::uumain` | uutils | 已是目标模式 |
| 96 | `mv` | `uu_mv::uumain` | uutils | 已是目标模式 |
| 97 | `nl` | `uu_nl::uumain` | uutils | 已是目标模式 |
| 98 | `nproc` | `uu_nproc::uumain` | uutils | 已是目标模式 |
| 99 | `numfmt` | `uu_numfmt::uumain` | uutils | 已是目标模式 |
| 100 | `od` | `uu_od::uumain` | uutils | 已是目标模式 |
| 101 | `paste` | `uu_paste::uumain` | uutils | 已是目标模式 |
| 102 | `pr` | `uu_pr::uumain` | uutils | 已是目标模式 |
| 103 | `printenv` | `uu_printenv::uumain` | uutils | 已是目标模式 |
| 104 | `ptx` | `uu_ptx::uumain` | uutils | 已是目标模式 |
| 105 | `readlink` | `uu_readlink::uumain` | uutils | 已是目标模式 |
| 106 | `realpath` | `uu_realpath::uumain` | uutils | 已是目标模式 |
| 107 | `rm` | `uu_rm::uumain` | uutils | 已是目标模式 |
| 108 | `rmdir` | `uu_rmdir::uumain` | uutils | 已是目标模式 |
| 109 | `seq` | `uu_seq::uumain` | uutils | 已是目标模式 |
| 110 | `sha1sum` | `uu_sha1sum::uumain` | uutils | 已是目标模式 |
| 111 | `sha224sum` | `uu_sha224sum::uumain` | uutils | 已是目标模式 |
| 112 | `sha256sum` | `uu_sha256sum::uumain` | uutils | 已是目标模式 |
| 113 | `sha384sum` | `uu_sha384sum::uumain` | uutils | 已是目标模式 |
| 114 | `sha512sum` | `uu_sha512sum::uumain` | uutils | 已是目标模式 |
| 115 | `shred` | `uu_shred::uumain` | uutils | 已是目标模式 |
| 116 | `shuf` | `uu_shuf::uumain` | uutils | 已是目标模式 |
| 117 | `sleep` | `uu_sleep::uumain` | uutils | 已是目标模式 |
| 118 | `sort` | `uu_sort::uumain` | uutils | 已是目标模式 |
| 119 | `split` | `uu_split::uumain` | uutils | 已是目标模式 |
| 120 | `sum` | `uu_sum::uumain` | uutils | 已是目标模式 |
| 121 | `sync` | `uu_sync::uumain` | uutils | 已是目标模式 |
| 122 | `tac` | `uu_tac::uumain` | uutils | 已是目标模式 |
| 123 | `tail` | patched `uu_tail::uumain` | uutils | 已是目标模式 |
| 124 | `tee` | `uu_tee::uumain` | uutils | 已是目标模式 |
| 125 | `touch` | `uu_touch::uumain` | uutils | 已是目标模式 |
| 126 | `tr` | `uu_tr::uumain` | uutils | 已是目标模式 |
| 127 | `truncate` | `uu_truncate::uumain` | uutils | 已是目标模式 |
| 128 | `tsort` | `uu_tsort::uumain` | uutils | 已是目标模式 |
| 129 | `uname` | `uu_uname::uumain` | uutils | 已是目标模式 |
| 130 | `unexpand` | `uu_unexpand::uumain` | uutils | 已是目标模式 |
| 131 | `uniq` | `uu_uniq::uumain` | uutils | 已是目标模式 |
| 132 | `unlink` | `uu_unlink::uumain` | uutils | 已是目标模式 |
| 133 | `vdir` | `uu_vdir::uumain` | uutils | 已是目标模式 |
| 134 | `wc` | `uu_wc::uumain` | uutils | 已是目标模式 |
| 135 | `whoami` | patched `uu_whoami::uumain` | uutils | 已是目标模式 |

## 3. 其余基础命令（28）

| # | 命令 | 当前实现 | 当前入口/解析 | 目标判断 |
|---:|---|---|---|---|
| 136 | `grep` | uutils/grep + Oniguruma | `uu_grep::uumain(argv)`；上游解析 | 已是目标模式 |
| 137 | `rg` | ripgrep 底层库 | 本地 `RipgrepCommand` + Clap | 应寻找/抽取官方 ripgrep CLI |
| 138 | `clear` | ANSI 控制序列 | 本地 Brush Command；只定义 `-x` | 可保留项目自有实现 |
| 139 | `which` | 本地 PATH/builtin 查询 | 本地 Brush Command + Clap | 可保留或寻找上游 CLI |
| 140 | `find` | patched uutils/findutils | `find_main(argv)`；上游解析 | 已是目标模式 |
| 141 | `xargs` | patched uutils/findutils | `xargs_main(argv)`；上游解析 | 已是目标模式 |
| 142 | `tree` | 本地文件遍历 | 本地 Brush Command + Clap | 应寻找可嵌入的上游 tree CLI |
| 143 | `diff` | patched uutils/diffutils | `diff::main(argv)`；上游解析 | 已是目标模式 |
| 144 | `cmp` | patched uutils/diffutils | `cmp::main(argv)`；上游解析 | 已是目标模式 |
| 145 | `gzip` | `flate2` 库 | 本地 Brush Command + Clap | 应寻找上游 gzip CLI/uumain |
| 146 | `gunzip` | `flate2` 库 | 本地 Brush Command + Clap | 应寻找上游 gunzip CLI/uumain |
| 147 | `sed` | uutils/sed | `sed::sed::uumain(argv)`；上游解析 | 已是目标模式 |
| 148 | `curl` | `ureq` HTTP 库 | 本地 Brush Command + Clap | 应 vendor/抽取成熟 curl CLI，或明确改名 |
| 149 | `wget` | `ureq` HTTP 库 | 本地 Brush Command + Clap | 应寻找上游 wget CLI，或明确改名 |
| 150 | `tar` | Rust `tar`/压缩库 | 本地 `TarCommand` + Clap | 应寻找上游 tar CLI/uumain |
| 151 | `zip` | Rust `zip` crate | 本地 Brush Command + Clap | 应寻找上游 zip CLI |
| 152 | `unzip` | Rust `zip` crate | 本地 Brush Command + Clap | 应寻找上游 unzip CLI |
| 153 | `sqlite3` | `rusqlite` | 本地 sqlite3 前端 + Clap | 应抽取/嵌入官方 sqlite3 CLI |
| 154 | `jq` | 官方 jq 1.8.2 C | patched `ys_jq_main(argc, argv)`；官方解析 | 已是目标模式 |
| 155 | `git` | `git2`/libgit2 | 本地解析子命令并映射库 API | 应评估 libgit2 CLI 层；无法完整替代官方 Git CLI 时明确子集 |
| 156 | `awk` | One True Awk C | patched `ys_awk_main(argc, argv)`；官方解析 | 已是目标模式 |
| 157 | `ssh` | `russh` | 本地参数解析 + 专用异步客户端 | 保留专用 Host；可统一外层接口 |
| 158 | `scp` | `russh-sftp` | 本地参数解析 + SFTP 传输 | 保留专用 Host；可统一外层接口 |
| 159 | `sftp` | `russh-sftp` | 本地参数解析 + 自建 REPL | 保留专用 Host；可统一外层接口 |
| 160 | `mosh` | 自研 Rust Mosh 客户端 | 本地参数解析 + SSH/UDP/协议栈 | 保留专用 Host；可统一外层接口 |
| 161 | `edit` | 自研 EditorCommand | 本地 Clap；Insert 模式 | 保留专用交互命令 |
| 162 | `vi` | 自研 EditorCommand | 本地 Clap；Normal 模式 | 保留专用交互命令，或换可嵌入上游 |
| 163 | `nano` | 自研 EditorCommand | 本地 Clap；Insert 模式 | 保留专用交互命令，或换可嵌入上游 |

## 4. Feature-gated 命令（8）

| # | Feature | 命令 | 当前实现 | 当前入口/解析 | 目标判断 |
|---:|---|---|---|---|---|
| 164 | `python` | `python3` | CPython XCFramework | `python_adapter` → `ys_python_run(argc, argv)` | 保留专用 Host；argv 继续交给 CPython |
| 165 | `python` | `python` | 同 `python3` | 同一个 CPython Host | 保留专用 Host |
| 166 | `python` | `pip` | CPython 内嵌 pip | Adapter → Python driver → pip | 保留专用 Host；flags 交给 pip |
| 167 | `python` | `pip3` | 同 `pip` | 同一个 pip 入口 | 保留专用 Host |
| 168 | `vision` | `ocr` | Apple Vision | Rust Adapter → Swift `ys_ocr_run` | 保留系统能力 Host |
| 169 | `node` | `node` | NodeMobile 常驻 Node | JSON 请求携带 argv/cwd/env/stdin | 保留专用 Host；flags 交给 Node |
| 170 | `node` | `npm` | 官方 `npm-cli.js` | 改写为 Node 执行官方 npm CLI | 已符合“复用上游 CLI”原则 |
| 171 | `node` | `npx` | 官方 `npx-cli.js` | 改写为 Node 执行官方 npx CLI | 已符合“复用上游 CLI”原则 |

## 5. 改造结论

### 已经符合“上游 argv 入口”模式

```text
74 个 uutils/coreutils
grep
sed
find
xargs
diff
cmp
awk
jq
npm
npx
```

共 84 个命令名。

### 不应该转成进程型 uutils 模式

61 个 Brush Bash builtins 中，大部分必须直接访问当前 Shell 的变量、cwd、
alias、函数、options、历史、补全或 AST 控制流。它们可以统一注册 API，
但不能通过临时 cwd/env/fd 快照模拟，否则状态修改无法留在当前 Shell。

### 最值得改造成上游 CLI 入口

```text
rg
tree
gzip
gunzip
curl
wget
tar
zip
unzip
sqlite3
git
```

其中 `rg` 最明确：当前已经依赖 ripgrep 的底层库，却在本地重新声明大量
Clap flags。

### 保留专用 Host，但统一外层协议

```text
ssh
scp
sftp
mosh
edit
vi
nano
python
python3
pip
pip3
node
ocr
```

它们不适合强行包装成普通同步 `uumain`，但对 Brush 的外层可以统一为：

```text
CommandContext {
    name,
    argv,
    cwd,
    env,
    stdin,
    stdout,
    stderr,
    cancellation,
    terminal
}
        ↓
CommandResult { exit_code }
```
