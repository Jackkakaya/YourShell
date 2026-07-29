# YourShell 命令接入清单

本文根据当前源码中的 `core/src/lib.rs::build_shell()`、Brush 默认 builtin
注册表和 uutils 注册表整理。

统计口径：

- 基础 Rust Core：163 个命令名。
- 开启 `python`、`node`、`vision` 三个 feature：171 个命令名。
- 同一个实现以多个名字注册时，每个用户可调用的名字分别计数。
- `yes` 和 `more` 存在于 uutils 依赖中，但没有注册，不计入可用命令。
- 本表初始统计基于接入清单；后续新增的 `stat`、`egrep`、`fgrep` 和
  iOS Host 别名已在下方明细及源码注册表中体现，数量应以
  `build_shell()` 当前注册结果为准。

## 1. 总览

| 类别 | 数量 | 接入模式 | 参数解析归属 |
|---|---:|---|---|
| Brush Bash builtins | 61 | `BuiltinSet::BashMode` 批量注册 | Brush |
| uutils/coreutils | 74 | 通用 `uutils_adapter` → 上游 `uumain(argv)` | uutils |
| uutils 专用 CLI | 6 | 专用 Adapter → 上游 CLI 入口 | 上游项目 |
| Vendored C CLI | 2 | 重命名并修补官方 `main(argc, argv)` | 上游 C CLI |
| 本地 Brush Command | 12 | `builtins::Command` + 本地 Clap | YourShell |
| 库 API / 自研客户端 | 5 | 本地解析 CLI，调用 Rust 库或自研协议 | YourShell |
| 自研编辑器别名 | 3 | 三个名字注册同一个编辑器 Command | YourShell |
| 嵌入式运行时和系统能力 | 8 | Adapter → CPython、NodeMobile、Vision | 上游运行时或 YourShell |

说明：表中的“uutils 专用 CLI”是 `grep`、`sed`、`find`、`xargs`、`diff`
和 `cmp`；`jq` 与 `awk` 单独归入 Vendored C CLI。各项的完整明细见下文。

## 1.1 进程状态与锁分类

| 类型 | 当前命令 | 是否经过 `process_state_lock` | 原因 |
|---|---|---:|---|
| Shell-native | Bash builtins | 否 | 直接访问 Brush Shell 状态 |
| Session-safe Host | `git`, `ssh`, `scp`, `sftp`, `mosh`, `ocr`, `curl`, `wget` | 否 | 直接使用 Session/Host I/O 或 ureq 请求 API |
| Session-safe iOS Host | `pbcopy`, `pbpaste`, `open`, `openurl` | 否 | 直接使用 `ExecutionContext` + UIKit callback |
| Process-shaped Native CLI | `awk`, `grep`, `rg`, `sed`, `find`, `xargs`, `diff`, `cmp`, `uutils` | 是 | 入口依赖进程 fd/cwd/env |
| Process-shaped vendored C | `tar`, `unzip`, `gzip`, `sqlite3`, `jq` | 是 | 上游 `main(argc, argv)` 使用 libc 全局状态 |
| Embedded runtime | `python`, `node/npm/npx` | 按 adapter | runtime 自身有独立 Host/状态模型 |

这张表是锁迁移的准入边界：没有显式 Session I/O 注入之前，不能因为命令
“看起来是纯 Rust”就绕过进程桥接。

## 2. Brush Bash builtins（61）

### 命令

```text
.
:
[
alias
bg
bind
break
builtin
caller
cd
command
compgen
complete
compopt
continue
declare
dirs
disown
echo
enable
eval
exec
exit
export
false
fc
fg
getopts
hash
help
history
jobs
kill
let
local
logout
mapfile
popd
printf
pushd
pwd
read
readarray
readonly
return
set
shift
shopt
source
suspend
test
times
trap
true
type
typeset
ulimit
umask
unalias
unset
wait
```

### 接入方式

```rust
brush_core::Shell::builder()
    .default_builtins(BuiltinSet::BashMode)
```

`brush-builtins` 内部建立：

```text
"cd"     → CdCommand::execute()
"export" → ExportCommand::execute()
"echo"   → EchoCommand::execute()
...
```

这些命令直接访问和修改 `brush_core::Shell`，适合实现 `cd`、`export`、
`read`、`set` 等必须影响当前 Shell 状态的能力。YourShell 不逐个实现，
只选择 `BuiltinSet::BashMode`。

注意：

- `disown` 和 `logout` 当前由 Brush 注册为未实现命令。
- `bg`、`fg`、`jobs`、`exec` 等命令受 iOS 无法 `fork/exec` 的环境约束。

## 3. uutils/coreutils（74）

### 命令

```text
arch
b2sum
base32
base64
basename
basenc
cat
cksum
comm
cp
csplit
cut
date
dd
df
dir
dircolors
dirname
du
env
expand
expr
factor
fmt
fold
head
hostname
join
link
ln
ls
md5sum
mkdir
mktemp
mv
nl
nproc
numfmt
od
paste
pr
printenv
ptx
readlink
realpath
rm
rmdir
seq
sha1sum
sha224sum
sha256sum
sha384sum
sha512sum
shred
shuf
sleep
sort
split
sum
sync
tac
tail
tee
touch
tr
truncate
tsort
uname
unexpand
uniq
unlink
vdir
wc
whoami
```

### 接入方式

`brush-coreutils-builtins::bundled_commands()` 提供：

```text
"ls"  → uu_ls::uumain(argv)
"cat" → uu_cat::uumain(argv)
"cp"  → uu_cp::uumain(argv)
...
```

`core/src/uutils_adapter.rs` 完成：

1. 获取命令注册表。
2. 过滤同名 Brush builtin 和明确禁用的命令。
3. 将每个名字注册为 Brush builtin。
4. 根据 `context.command_name` 找到相应 `uumain`。
5. 通过 `command_host` 映射 cwd、env 和 fd 0/1/2。
6. 原样传递 argv，由 uutils 解析所有 flags。
7. 恢复进程状态并把退出码返回 Brush。

### 使用 Brush 版本替代的 uutils 命令

以下 6 个命令在 uutils 中也存在，但实际注册 Brush 版本：

```text
echo
printf
pwd
test
true
false
```

### 明确不注册的 uutils 命令

| 命令 | 原因 |
|---|---|
| `yes` | 无限输出，当前中断传递不足以保证可靠停止 |
| `more` | 依赖传统交互式 TTY/pager 行为 |

## 4. 复用上游 CLI 的专用 Adapter

| 命令 | 底层实现 | 接入入口 | flags 解析 | 本地改动 |
|---|---|---|---|---|
| `grep` | uutils/grep + Oniguruma | `uu_grep::uumain(argv)` | 上游 | 重置 uutils 进程级退出码；通用 I/O 映射 |
| `sed` | uutils/sed | `sed::sed::uumain(argv)` | 上游 | 将裸 `-i` 规范化为 `--in-place=`，规避上游解析问题 |
| `find` | vendored uutils/findutils | `findutils::find::find_main()` | 上游 | `-exec` 改为调用进程内子 Shell |
| `xargs` | vendored uutils/findutils | `findutils::xargs::xargs_main()` | 上游 | 外部命令执行改为进程内子 Shell |
| `diff` | vendored uutils/diffutils | `diffutilslib::diff::main(argv)` | 上游 | `exit()` 改返回码，入口接受注入 argv |
| `cmp` | vendored uutils/diffutils | `diffutilslib::cmp::main(argv)` | 上游 | `exit()` 改返回码，入口接受注入 argv |

这些 Adapter 不重新声明完整 flags。YourShell 主要负责：

```text
Brush argv/cwd/env/fd
        ↓
command_host
        ↓
上游 CLI 入口
        ↓
exit code
```

## 5. Vendored C CLI

| 命令 | 底层实现 | 接入方式 | flags/语言解析 | 主要补丁 |
|---|---|---|---|---|
| `awk` | One True Awk | 官方入口重命名为 `ys_awk_main(argc, argv)` | 官方 awk | 避免 `exit()`；每次调用重置全局状态；禁用 `system()` 等进程能力 |
| `jq` | jqlang/jq 1.8.2 | 官方入口重命名为 `ys_jq_main(argc, argv)` | 官方 jq | `exit()` 改返回；不关闭宿主 stdout；清理长驻进程状态 |

两者都通过 `command_host` 临时映射当前会话的 cwd、环境和 fd 0/1/2。

## 6. 本地 Brush Command

| 命令 | 文件 | 底层能力 | 接入方式 | flags 解析评价 |
|---|---|---|---|---|
| `clear` | `builtins_extra.rs` | ANSI 控制序列 | Brush `Command` | 项目自有小命令，本地 Clap 合理 |
| `which` | `commands_ext.rs` | Brush builtin/PATH 查询 | Brush `Command` | 本地 Clap；可评估复用上游实现 |
| `tree` | `commands_ext.rs` | Rust 文件遍历 | Brush `Command` | 本地重建 tree CLI |
| `gzip` | `commands_ext.rs` | `flate2` | Brush `Command` | 本地重建 gzip CLI |
| `gunzip` | `commands_ext.rs` | `flate2` | Brush `Command` | 本地重建 gunzip CLI |
| `curl` | `commands_ext.rs` | `ureq` | Brush `Command` | 本地实现部分 curl CLI |
| `wget` | `commands_ext.rs` | `ureq` | Brush `Command` | 本地实现部分 wget CLI |
| `tar` | `commands_ext.rs` | `tar`、`flate2`、`bzip2`、`xz2` | 自定义 Brush Registration，内部调用本地 `TarCommand` | 本地重建 tar CLI |
| `zip` | `commands_ext.rs` | Rust `zip` crate | Brush `Command` | 本地重建 zip CLI |
| `unzip` | `commands_ext.rs` | Rust `zip` crate | Brush `Command` | 本地重建 unzip CLI |
| `sqlite3` | `commands_ext.rs` | `rusqlite` | Brush `Command` | 使用真实 SQLite 引擎，但 CLI 是本地实现 |
| `rg` | `ripgrep_cmd.rs` | `ignore`、`grep-regex`、`grep-searcher`、`grep-printer` | Brush `Command` | 本地用 Clap 重建 ripgrep CLI；应优先整改 |

### `rg` 的特殊说明

当前 `rg` 复用了 ripgrep 搜索、遍历和输出库，但没有复用官方完整 CLI：

```text
本地 Clap flags
      ↓
手动映射
      ↓
ripgrep 底层库
```

长期更适合 vendor/patch ripgrep CLI 层，形成可调用的：

```text
upstream_rg::run(argv, io) -> exit_code
```

## 7. 库 API 或自研客户端

| 命令 | 底层实现 | 接入方式 | 参数解析 | 覆盖范围 |
|---|---|---|---|---|
| `git` | `git2`/libgit2 | 本地解析 Git 子命令并映射到 libgit2 API | YourShell | `init`、`clone`、`status`、`add`、`commit`、`log`、`diff`、`branch`、`checkout`、`fetch`、`pull`、`push`、`config`、`remote` |
| `ssh` | `russh` | 自建 SSH CLI 和交互循环，直接使用会话 fd | YourShell | 常用连接、密钥、密码、远程命令和交互 Shell |
| `scp` | `russh-sftp` | 自建 scp 参数和文件传输 | YourShell | 基于 SFTP 子系统，不是 OpenSSH scp 完整实现 |
| `sftp` | `russh-sftp` | 自建 sftp 参数和行式 REPL | YourShell | `pwd`、`lpwd`、`cd`、`lcd`、`ls`、`lls`、`get`、`put`、`mkdir`、`rmdir`、`rm`、`rename` 等 |
| `mosh` | 自研 Rust Mosh 客户端 | SSH bootstrap + UDP/加密/状态同步协议 | YourShell | 客户端实现，不依赖外部 `mosh-client` 进程 |

这组命令不是简单包装现成 CLI，而是在库 API 或协议之上重建用户界面，
因此 CLI 兼容性和维护成本最高。

## 8. 自研编辑器

| 命令 | 实现 | 接入方式 | 参数解析 |
|---|---|---|---|
| `vi` | `editor::EditorCommand`，默认 Normal 模式 | Brush `Command` | 本地 Clap，只接收文件 |
| `edit` | 同一 `EditorCommand`，默认 Insert 模式 | Brush `Command` | 本地 Clap，只接收文件 |
| `nano` | 同一 `EditorCommand`，默认 Insert 模式 | Brush `Command` | 本地 Clap，只接收文件 |

三个名字共享同一个 ANSI 全屏编辑器引擎，不是上游 Vim、nano 或 nextvi CLI。

## 9. Feature-gated 嵌入式运行时和系统能力

| Feature | 命令 | 底层实现 | 接入方式 | 参数解析 |
|---|---|---|---|---|
| `python` | `python`, `python3` | CPython XCFramework | Rust Adapter → `python_host.c` → 常驻 CPython | CPython/Python driver |
| `python` | `pip`, `pip3` | 嵌入式 pip | Rust Adapter → CPython → pip module | pip |
| `node` | `node` | NodeMobile | Rust Adapter 经 loopback JSON 协议调用常驻 Node | Node |
| `node` | `npm` | 官方 `npm-cli.js` | 转换为 Node 执行官方 npm CLI | npm |
| `node` | `npx` | 官方 `npx-cli.js` | 转换为 Node 执行官方 npx CLI | npx |
| `vision` | `ocr` | Apple Vision | Rust Adapter → Swift `OCRHost` | YourShell，仅解析图片路径 |

## 10. 接入质量分类

### 方向正确：复用上游完整命令入口

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
python
python3
pip
pip3
node
npm
npx
```

### Shell 或 YourShell 自有能力

```text
61 个 Brush Bash builtins
clear
ocr
```

### 有现实理由自行实现，但应明确兼容边界

```text
ssh
scp
sftp
mosh
edit
vi
nano
```

### 本地重建成熟 CLI，后续应优先评估上游 CLI 接入

```text
rg
curl
wget
tar
zip
unzip
sqlite3
git
```

### 规模较小，仍可评估上游替代

```text
which
tree
gzip
gunzip
```

## 11. 相关源码入口

```text
core/src/lib.rs                   命令总注册入口
core/src/command_host.rs          进程型 CLI 的 cwd/env/fd 通用桥
core/src/uutils_adapter.rs        74 个 coreutils 的统一 Adapter
core/src/grep_adapter.rs          grep
core/src/sed_adapter.rs           sed
core/src/findutils_adapter.rs     find、xargs
core/src/diffutils_adapter.rs     diff、cmp
core/src/awk_adapter.rs           awk
core/src/jq_adapter.rs            jq
core/src/ripgrep_cmd.rs           rg 本地 CLI
core/src/commands_ext.rs          which/tree/archive/network/sqlite
core/src/git_adapter.rs           git
core/src/ssh_adapter.rs           ssh
core/src/sftp_adapter.rs          scp、sftp
core/src/mosh_adapter.rs          mosh
core/src/editor.rs                vi、edit、nano
core/src/python_adapter.rs        python、pip
core/src/node_adapter.rs          node、npm、npx
core/src/ocr_adapter.rs           ocr
```
