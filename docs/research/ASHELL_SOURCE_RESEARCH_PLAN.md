# YourShell 命令上游接入调研与改进计划

> 2026-07-28 重新调研版。只以 a-Shell 官方仓库、各上游官方仓库和
> YourShell 当前源码为依据。没有采用第三方介绍页面。

## 先说结论

目标不应写成“全部改成 uutils”，而应写成：

> 为所有普通 CLI 建立同一种薄适配协议：注册命令名，传入原始 argv、
> cwd、env 和 stdio，直接调用上游完整 CLI 入口；YourShell 不再重新定义
> 上游 flags。

uutils 只是该协议最成功的一组实现，不是所有命令的实现来源。Shell
stateful builtins、终端 UI、运行时和 iOS Host 命令必须保留专用入口。

## a-Shell 的源码事实

| 项目 | 官方源码显示的实际做法 | 对 YourShell 的启示 |
|---|---|---|
| `wget` | **当前没有该命令**。`Resources/bin`、`commandDictionary.plist`、`extraCommandsDictionary.plist`、`createFakeCommands.sh` 和 `a-Shell-commands/list` 均无 `wget` | 不能再引用“a-Shell 的 wget 实现”；要么接真实 Wget/Wget2 CLI，要么明确把它定义成有限兼容层 |
| `curl` | `curl_ios.framework/curl_ios` 的 `curl_main` | 完整上游 CLI 入口，注册层很薄；这是网络命令的正确参考 |
| `tar` | `tar.framework/tar` 的 `tar_main`；a-Shell credits 指向 libarchive | 优先直接嵌入 `bsdtar` CLI，而不是用 Rust `tar` 库重写 flags |
| `gzip/gunzip` | `files.framework/files` 的 `gzip_main` | 也是完整 CLI 入口，不是 a-Shell 自己解析 gzip flags |
| `bc/dc` | `bc_ios.framework/bc_ios` 的 `bcdc_main`；二进制依赖来自 `holzschu/bc`，其来源是 Gavin Howard bc 的移植 | 可移植上游 CLI 时，让上游自己解析 argv |
| `tree` | a-Shell 当前确实包含 `tree`，credits 指向 Steve Baker tree | 可以评估原生移植，但要先处理 GPL 和全局状态，不应继续手写子集 |
| `sqlite3` | 不在内置 framework 注册表；在 `a-Shell-commands` 包清单中 | a-Shell 选择了可安装命令，不代表 YourShell 必须用 WASM；SQLite 官方 `shell.c` 可直接嵌入 |
| `rg` | 不在内置 framework 注册表；在 `a-Shell-commands` 包清单中 | a-Shell 用 WASM 解决可执行文件问题；YourShell 可优先直接复用 ripgrep binary crate 源码入口 |
| `zip`/`xz` | 在 `a-Shell-commands` 包清单中 | 这是 a-Shell 的部署选择，不应自动成为 YourShell 的架构选择 |
| `ssh/scp/sftp` | `ssh_cmd.framework` 的多个上游入口 | 网络会话类仍可使用统一调用协议，但实现需拥有会话、TTY、取消等能力 |
| `ping/dig/nc/...` | `network_ios.framework` 的不同 `*_main` | 一套框架可以批量暴露多个完整 CLI，类似 uutils 模式 |
| App 命令 | `downloadFile`/`downloadFolder` 等名字出现在资源目录，但不在通用 CLI 注册表中 | 它们不是 wget 的替代实现，也不应与 POSIX/Unix CLI 混为一类 |

## 当前实现逐项判断

下表针对目前偏离“上游完整 CLI + 薄适配”的命令。Brush builtins 和已经采用
上游 dispatch 的 uutils/findutils/diffutils/sed/awk/jq 不在本轮重写范围。

| 命令 | 当前实现 | 是否符合预期 | 调研后的目标 | 优先级 |
|---|---|---:|---|---:|
| `rg` | `ripgrep_cmd.rs` 自己定义大量 clap flags，再拼 ripgrep 下层 crates | 否 | vendor 固定版本的 ripgrep binary crate 源码，把其 CLI runner 暴露成可调用入口；只写 argv/stdio/cwd bridge | P0 |
| `curl` | `commands_ext.rs` 用 HTTP 库重写一小部分 curl flags | 否 | 嵌入 curl 官方 tool 源码，调用 `curl_main` 等价入口；验证 iOS TLS、stdio、全局清理和取消 | P0 |
| `sqlite3` | `rusqlite` 上手写极简 SQL shell | 否 | 编译 SQLite 官方 `shell.c + sqlite3.c`，重命名/包装 `main`；保留完整 dot commands、输出模式和 argv | P0 |
| `tar` | Rust `tar`/压缩库 + 自定义 clap | 否 | 采用 libarchive `bsdtar` CLI；直接传 argv；补 session cwd/stdio bridge | P0 |
| `gzip`/`gunzip` | `flate2` + 自定义 clap | 否 | 首选轻量上游 gzip CLI 入口；先比较 ios_system `gzip_main` 的可复用性与独立上游维护成本 | P1 |
| `tree` | 自己遍历目录并定义 flags | 否 | 评估 Steve Baker tree 的原生入口；GPL 可接受才采用。若不可接受，寻找兼容许可证的完整 CLI，不能无声冒充完整 tree | P1 |
| `zip` | `zip` crate + 自定义 clap | 否 | 调研 Info-ZIP `zip` 的可嵌入入口和许可证；若移植成本不可控，再评估独立的兼容实现，不默认引入 WASM | P1 |
| `unzip` | `zip` crate + 自定义 clap | 否 | 优先 libarchive `bsdunzip` CLI；对照 Info-ZIP 常用行为做兼容测试 | P1 |
| `wget` | HTTP 库 + 自定义 wget 子集 | 否 | a-Shell 没有可参考实现。先做 GNU Wget/Wget2 的许可证、fork/exec、resolver、TLS 和全局状态移植 spike；不通过则删除“完整 wget”承诺，明确为 curl 兼容包装或暂不提供 | P1 |
| `git` | `git2` 上手写少数 subcommands 和 flags | 否 | 完整 Git CLI 不是 libgit2 的薄包装。调研上游 Git 在无 fork/exec 环境的改造边界；在结论前不继续扩写假 Git | P1 |
| `which` | 很小的项目实现 | 基本符合 | 它依赖 Shell 自己的 builtin/function/PATH 解析，保留项目实现；补 Bash 行为测试即可 | P2 |
| `clear` | 输出 ANSI，只有 `-x` | 符合 | 保留。它是终端能力，不值得引入完整 terminfo CLI | 保留 |
| `ssh/scp/sftp/mosh` | Rust 专用 adapter | 部分符合 | 先按协议能力审计，不以“必须有 main”为目标；这些命令的会话、PTY、取消和网络生命周期比 flag 复用更重要 | P2 |
| `python/pip/node/npm/npx` | 常驻运行时 Host | 符合其领域 | 保留专用 Host；统一外围 argv/env/cwd/stdio 接口，不改成 uutils | 保留 |
| `vi/nano/edit` | 项目内编辑器 | 产品选择 | 不是普通一次性 CLI；是否替换应作为编辑器产品决策，不能混入本轮 argv 标准化 | 独立 |
| `ocr` | iOS Vision Host | 符合 | 平台能力，无可转发的 Unix CLI main | 保留 |

## 标准化接口

所有“普通 CLI”最终只需要适配以下接口，不再为每个命令写 clap：

```text
command name
    -> upstream entry(argc, argv)
    -> invocation context { cwd, env, stdin, stdout, stderr, cancel }
    -> exit status
    -> reset invocation-local / upstream global state
```

实现上允许三种 entry：

1. Rust runner：上游直接暴露 `run(args, io, cwd)`。
2. C ABI main：把上游 `main` 重命名为 `xxx_main`，由统一 Host 调用。
3. Stateful/Host registration：只用于必须修改 Shell 状态或持有平台会话的命令。

三者共享注册表和 invocation context，但不强迫底层语言、线程模型或状态模型相同。

## 执行计划

| 阶段 | 工作 | 交付/退出条件 |
|---|---|---|
| 0. 建基线 | 为上述命令采集真实 CLI 的 `--help`、常用 argv、stdio、exit code 和文件副作用测试 | 每个待替换命令有兼容测试；不能只测“能跑” |
| 1. 通用 Host | 从现有 uutils/command_host 提取统一 argv/cwd/env/stdio/exit bridge；设计显式的全局状态清理钩子 | 新 CLI 接入不需要定义 flags；适配文件只负责注册和入口调用 |
| 2. 三个 POC | 分别接 `sqlite3 shell.c`、libarchive `bsdtar`、上游 ripgrep CLI | 覆盖 C main、复杂 C CLI、Rust binary CLI 三种代表；证明不需要 WASM |
| 3. 网络 CLI | 接官方 curl tool；专项验证 TLS、重定向、上传、header、取消和并发 session | 替换手写 curl；session 间无 stdio/env/cwd 串扰 |
| 4. 压缩组 | 接 gzip/gunzip、bsdunzip，再评估 zip | 删除对应手写 clap 和归档实现；行为测试通过 |
| 5. tree/wget/git 决策 | 完成许可证与可移植性 spike | 每项形成 Adopt / Limited compatibility / Do not ship 决策，不以凑命令数为目标 |
| 6. 清理 | 删除已替换的 `commands_ext.rs` 和 `ripgrep_cmd.rs` 对应代码、重复依赖和重复测试 | 普通 CLI adapter 保持薄；171 工具表同步标记来源与入口 |

## 实施进度

### 2026-07-28：SQLite POC 完成

- 已删除 `commands_ext.rs` 中手写的 `SqliteCommand`、SQL 分句和结果格式化。
- 已接入 SQLite 3.46.0 官方 `shell.c`；Rust adapter 不解析任何 sqlite3 flag。
- 复用 `rusqlite/libsqlite3-sys` 已链接的同版本 SQLite 引擎，没有重复编译
  `sqlite3.c`。
- `command_host` 现会在恢复 fd 前调用 C `fflush(NULL)`。这是后续
  bsdtar/curl 等 C CLI 共用的 Host 修复，避免 C stdio 缓冲写入下一条命令。
- Host 捕获上游 `exit()`，并在每次调用后 reset SQLite 全局状态；
  `.exit 7` 只结束当前命令，下一次 sqlite3 调用仍可正常执行。
- iOS 禁止的 `system()` 由 Host 返回 `ENOSYS`；没有改写 SQLite CLI parser。
- SQLite 的 10 个行为测试全部通过：内存/文件数据库、stdin、header、字符串
  分号、CSV、JSON、`.schema`、非法参数、`.exit` 和重复调用。
- `cargo check --target aarch64-apple-ios` 通过。
- 完整 host battery 为 303/309；剩余 6 项全部是既有 jq/awk
  `yyparse/yylex/yyerror` 符号冲突导致的 jq parser 失败，不是 SQLite 回归。

### 2026-07-28：bsdtar POC 完成

- 已删除手写 `TarCommand` 和 `tar_registration()`；项目不再定义 tar flags。
- 接入 libarchive 3.8.8 官方 `bsdtar` 前端，adapter 只转 argv 并进入通用 Host。
- macOS/iOS 直接链接 Apple 公开的 `libarchive`，不重复 vendor 整个归档引擎；
  仅补入 Apple 当前版本尚未导出的上游 public-domain 日期解析函数。
- Host 用 thread-local `setjmp` 捕获 `exit()`，并恢复 bsdtar 修改的 signal handler。
- 已删除 Rust `tar`、`bzip2`、`xz2` 三个直接依赖。
- gzip/bzip2/xz、stdin/stdout 管道、exclude/strip-components、错误后重复调用
  共 7 项全部通过；macOS 和 `aarch64-apple-ios` 均通过 `cargo check`。
- 同轮修复了 jq 与 one-true-awk 的 yacc 默认符号冲突：只给 jq 生成的
  `yyparse/yylex/yyerror` 加前缀，不修改语法。
- 完整 battery 现为 **314/314**；4 sessions × 25 rounds 并发隔离测试通过。

### 下一步：ripgrep

1. 固定与当前 `grep-*` crates 兼容的 ripgrep 上游版本。
2. 将 upstream binary crate 的 CLI runner 提取为 vendored workspace crate；
   保持上游 clap 定义和参数处理，不复制 flags 到 adapter。
3. 把 process exit、signal、pager/decompress external command 路径改为可注入 Host
   能力；iOS 不支持的外部进程路径明确报错。
4. adapter 只传 argv/cwd/env/stdio/cancel。
5. 对照官方 `rg --help` 建行为测试后删除当前 `ripgrep_cmd.rs`。

### 2026-07-28：ripgrep POC 完成

- 固定 ripgrep 15.1.0，并 vendor 官方 `crates/core` CLI 前端。
- 上游仅增加 argv 注入、logger 单次初始化、每次调用 error-state reset，以及
  禁止写错误时 `process::exit` 四个 Host seam；flags/search/output 均未重写。
- 删除 616 行手写 `ripgrep_cmd.rs`，换成薄 `ripgrep_adapter.rs`。
- 删除原实现直接使用的旧 `grep-searcher`、`grep-regex`、`grep-printer`、
  `ignore`、`termcolor` 依赖；由官方 CLI 自己声明其版本。
- 现有 20 个 rg 行为测试全部通过，包括递归、gitignore、hidden、glob/type、
  stdin、exit code、replace/context/files/max-depth。
- macOS 与 `aarch64-apple-ios` 均通过 `cargo check`；完整 battery 仍只剩
  原有 6 个 jq parser 冲突。

### 2026-07-29：curl 边界调研完成，暂不机械替换

- 官方 curl 当前发布线已到 8.20；curl CLI 本身可以提供
  `curl_main(argc, argv)`，所以 argv 接入形式是可行的。
- 但 iOS SDK 没有公开 libcurl；只有 BoringSSL 系统库。a-Shell 的
  ios_system 实际携带 curl 8.1.2 的完整 tool + libcurl 源码，并依赖其
  自己的 OpenSSL/libssh2 framework，不能作为 YourShell 的“一行转发”。
- 当前正确的下一步是做一个独立 curl build spike：裁剪协议（HTTP/HTTPS、
  file）并明确 TLS backend、CA 路径、SSH/FTP 是否支持，再决定 Adopt
  官方 CLI 或保留当前 `ureq` 的有限兼容层。不能在没有 TLS/CA 结论前替换，
  否则会把现有可用下载能力换成不可验证的 iOS 网络栈。

### 2026-07-29：压缩组初步调研

- iOS SDK 提供 `libz.tbd` 和 `libcompression.tbd`，所以 gzip 的底层压缩能力
  可复用系统库；这和 curl 的 libcurl 情况不同。
- GNU gzip CLI 仍是独立 C 前端，若接入应沿用 `bsdtar` 方案：vendor 官方
  `gzip` CLI 的必要源码，`main(argc, argv)` 只经过 Host，链接系统 `libz`。
- GNU gzip 1.14 已确认是 GPL-3.0，且依赖较重的 gnulib；不直接 vendor，避免
  把当前工程引入不必要的 copyleft 和构建负担。
- OpenBSD/FreeBSD 提供 BSD 许可的 gzip/gunzip 兼容实现；下一步改以 BSD
  版本做 build spike，保留同样的官方 argv/Host 接入路线。
- 当前手写 gzip/gunzip 仍覆盖有限 flags，不能在 BSD CLI 的完整行为测试前删除。
- `zip/unzip` 不能共用 libz：zip 容器、权限、符号链接和安全路径处理需要
  独立 CLI/库审计，暂不把 Rust `zip` crate 误标成官方 CLI 接入。

### 2026-07-29：BSD gzip POC 完成

- GNU gzip 1.14 因 GPL-3.0 和 gnulib 依赖未采用。
- 接入 FreeBSD/NetBSD-derived BSD-2-Clause gzip frontend，保留官方 getopt
  行为，禁用 iOS 当前不需要的 bzip2/xz/zstd/compress/pack 格式。
- 底层直接链接 Apple `libz`；adapter 只负责 argv 和 Host 调用。
- 为重复调用增加了 argv[0] 识别（`gzip`/`gunzip`）和 invocation 状态 reset，
  并用 setjmp 捕获上游 exit。
- 删除旧手写 gzip/gunzip 实现；gzip roundtrip 和 stdin pipeline 均通过。
- 完整 battery 仍为 **314/314**，`aarch64-apple-ios` 编译通过。

### 2026-07-29：bsdunzip POC 完成

- 接入 libarchive 3.8.8 官方 `bsdunzip` 前端（BSD-2-Clause），adapter 只做
  argv/cwd/stdio 转换并通过 Host 捕获 `exit`。
- 复用已接入的 Apple `libarchive`；没有复制 ZIP 解压逻辑，也没有新增一套
  Rust flag 定义。旧手写 `UnzipCommand` 已删除。
- 现有 `zip-unzip-roundtrip`、`unzip-list` 行为测试通过，macOS 与
  `aarch64-apple-ios` 编译通过。
- `zip` 仍保留现有 Rust 创建器：`bsdunzip` 只负责解压，不能把创建语义
  偷换成 `bsdtar -a`；下一步单独审计 zip CLI 兼容性和是否存在合适的上游
  ZIP 创建前端。

### 2026-07-29：zip 创建器行为审计

- 为现有创建器补充 `-j/-x/-u/-d/-0` 行为测试；全部通过，当前 battery 为
  **318/318**。
- 发现并修复 bsdunzip 嵌入后的真实问题：官方前端的静态 flag 状态按进程
  生命周期保存，连续调用会让上一次的 `-l` 污染后续 `-p`。在官方
  `main` 入口增加 invocation reset 后，重复调用和 iOS 编译均通过。
- 结论：解压已是 upstream CLI；创建暂不替换。Info-ZIP/bsdtar 创建端的
  参数、目录项、权限和更新删除语义仍需独立兼容性评估，不能仅凭“都有 ZIP”
  就机械转发。

### 2026-07-29：Info-ZIP zip 上游调研

- Info-ZIP Zip 3.0 是成熟的 BSD-like Info-ZIP License 实现，确实提供传统
  `zip` CLI；但源码规模约 50k 行 C，并不是一个可单文件转发的小前端，
  还带 Unix/平台、Zip64、加密、压缩后端等条件编译。
- 已确认 libarchive/bsdtar 可以创建 ZIP，但其官方语义是 tar 风格的
  `-a -cf`，不是 Info-ZIP `zip`；尤其更新、删除、junk-paths 和默认递归
  规则不能直接映射。
- 因此下一步不是把 `zip` 粗暴别名到 `bsdtar`，而是做 Info-ZIP 的最小
  build spike：只启用 deflate/store、普通文件/目录、Zip64，关闭加密和
  外部压缩器，验证 iOS 编译、重复调用状态 reset，再用现有行为测试比较。

### 2026-07-29：zip 最终方案确定

- `zip` 创建端继续使用当前 Rust `zip` crate，不再移植 Info-ZIP。
- 原因：Rust 实现已在 iOS 目标编译通过，当前行为测试覆盖创建、递归、
  junk paths、exclude、update、delete、store，并且没有进程全局状态或
  外部进程依赖。
- `unzip` 继续使用 libarchive 官方 `bsdunzip`，形成“创建用纯 Rust、
  解压用上游 CLI”的明确边界；两者通过 ZIP 格式行为测试互相验证。
- 后续只做增量增强：Zip64、大文件、符号链接、权限/时间戳和 stdin 输入；
  不再为了形式统一替换成熟的 iOS-safe 实现。

### 2026-07-29：低风险 grep 兼容名

- `egrep` 和 `fgrep` 复用 uutils grep adapter，通过 argv[0] 注入固定的
  `-E`/`-F` 模式，不新增第二套 parser。
- 两个别名行为测试通过；完整 battery 当前为 **321/321**。
- 当前 Brush 的 curated uutils bundle 没有 `stat`，已直接接入官方 `uu_stat`
  0.8.0 crate，并复用现有 process-shaped uutils adapter；没有手写 parser。
- `stat -c` 行为测试通过，battery 当前为 **322/322**，iOS target 编译通过。
- flag coverage 审计也已修复空 flag 列表的除零边界；当前完整报告可运行，
  egrep/fgrep 别名显示 0/0（100%），curl 的 `--connect-timeout` 和
  `--max-time` 已通过 ureq 3.3 的请求级 timeout API 接入，覆盖率达到 17/17。

### 2026-07-29：iOS Host ABI 初版

- 新增可选 `ashell_ios_host_install` ABI，Rust core 不依赖 UIKit；Host 未注册
  时 `pbcopy/pbpaste/open/openurl` 明确返回 127。
- App 侧新增 UIKit 实现：剪贴板读写和 `UIApplication.open`，并在
  `ShellSession` 初始化时注册回调。
- `cargo check`、`aarch64-apple-ios` 和 battery **321/321** 通过。
- Xcode 模拟器构建已进入链接阶段，但当前工程缺少已有的
  `core/target/aarch64-apple-ios-sim/release/libashellcore`，导致原有
  `ashell_*` 符号整体无法链接；这不是 Host ABI 编译错误，需先构建对应
  simulator Rust 静态库后再做 App 端最终验证。
- 已构建 `cargo build --target aarch64-apple-ios-sim --release`，补齐该静态库。
  arm64 simulator 的 Xcode 构建随后进入 Swift Package 解析阶段，但本次
  `xcodebuild` 在解析/构建阶段长时间无输出被停止；Rust 和 Swift 源码层面
  的 ABI 错误已排除。x86_64 simulator 仍无法验证，因为本机未安装
  `x86_64-apple-ios` Rust target。
- 为 App target 补上系统 `libarchive` 链接后，指定 arm64 iOS Simulator 的
  `xcodebuild -skipPackageUpdates build` 已成功；这证明 UIKit Host ABI 和
  新增命令可以与静态 Rust core 完成最终链接。
- 模拟器已安装并启动该 App，系统日志未发现 `pbcopy` Host 崩溃或 fatal 错误；
  由于当前 headless 启动通道不会稳定返回终端 transcript，剪贴板内容级验证
  仍需 UI 自动化或专用测试入口补齐。

### 2026-07-29：network_ios C1 调研完成

- 官方 `network_ios` 提供 `ping/nc/nslookup/host/dig/telnet/whois`，许可证为
  BSD-3-Clause；但它的交付形态是给 ios_system 动态加载的
  `network_ios.xcframework`，不是可直接调用的 `main/uumain` CLI crate。
- README 明确要求把 framework 嵌入 App，由 ios_system 在命令名出现时加载；
  这与 YourShell 当前静态 Rust core + 显式 Host ABI 不同，不能直接写一个
  薄 adapter 假装完成接入。
- 后续若采用，应单独建立 Swift/ObjC framework Host seam，并处理动态加载、
  socket 权限、取消和 Session I/O；暂不引入 WASM 或复制其动态加载机制。

## 必须单独解决的并发问题

“调用上游 main”常会碰到进程全局的 `cwd`、`environ`、stdio、`getopt` 和上游
静态变量。不能因为入口统一就默认它线程安全。

- 同一 session 本来就应串行执行有状态 Shell 命令。
- session 之间应并发；只有确实触碰进程全局状态的某个 Host 需要细粒度隔离或锁。
- 每个候选上游在 Adopt 前必须列出全局状态，并提供 reset 或隔离策略。
- 现有全局锁问题继续保留为架构 TODO，不能用“一把全局锁”作为最终适配方案。

## 官方证据

- [a-Shell 命令注册表](https://github.com/holzschu/a-shell/blob/master/Resources/commandDictionary.plist)
- [a-Shell 额外命令注册表](https://github.com/holzschu/a-shell/blob/master/Resources/extraCommandsDictionary.plist)
- [a-Shell framework 依赖清单](https://github.com/holzschu/a-shell/blob/master/xcfs/Package.swift)
- [a-Shell 内置命令占位清单](https://github.com/holzschu/a-shell/blob/master/createFakeCommands.sh)
- [a-Shell 可安装包清单](https://github.com/holzschu/a-Shell-commands/blob/master/list)
- [ios_system](https://github.com/holzschu/ios_system)
- [network_ios](https://github.com/holzschu/network_ios)
- [curl 官方源码](https://github.com/curl/curl)
- [libarchive：bsdtar/bsdunzip](https://github.com/libarchive/libarchive)
- [SQLite 官方 CLI 与 shell.c](https://www.sqlite.org/cli.html)
- [ripgrep 官方源码](https://github.com/BurntSushi/ripgrep)
