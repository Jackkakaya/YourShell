# 上游 CLI 复用调研计划

> 修正说明：本文件中的 WASI 内容只保留为候选技术调研，不再作为建议的默认
> 架构或实施前置。以源码审计后的
> [`CURRENT_COMMAND_AUDIT.md`](./CURRENT_COMMAND_AUDIT.md) 为当前结论：
> 优先沿用现有 Native 上游入口模式，只有具体命令证明 Native 路线不可接受时
> 才重新评估 WASM。

目标：尽量停止在 YourShell 中手写成熟命令的 flags 和 CLI 行为。优先直接
复用开源项目的参数解析、命令实现和退出码，只维护一层与 Brush Session 的
argv/cwd/env/I/O 适配。

## 目标接入形态

```text
Brush CommandContext
        ↓
YourShell 通用 Adapter
        ↓
upstream main/uumain/run(argv, io)
        ↓
exit code
```

允许对上游做小型、可审计的补丁：

- `process::exit()` / `exit()` 改为返回退出码；
- `std::env::args()` / `argc, argv` 改为接受调用方 argv；
- stdin/stdout/stderr 改为注入或通过过渡 Host 映射；
- fork/exec 改为 YourShell 进程内子 Shell hook；
- 清理可重复调用所需的进程全局状态。

不接受：

- 在 YourShell 重新声明上游已有的大量 flags；
- 静默接受但不实现 flags；
- 为兼容 CLI 而引入无法在 iOS 使用的 fork/exec/JIT；
- 未隔离的 `exit()`、signal handler 或永久进程全局状态修改。

## 评估字段

| 字段 | 含义 |
|---|---|
| 上游候选 | 优先官方项目，其次活跃且许可证兼容的实现 |
| 可调用入口 | 是否已有 `main`、`uumain`、`run` 或可抽取 CLI library |
| I/O 可注入 | 是否能避免进程全局 fd 0/1/2 |
| iOS 阻碍 | fork/exec、signal、PTY、termios、平台 API、JIT 等 |
| 可重入性 | 同一 App 进程内能否反复调用 |
| 许可证 | 是否适合当前项目分发 |
| 补丁等级 | S：直接接；A：小补丁；B：中等抽取；C：高风险/不建议 |
| 决策 | Adopt、Prototype、Keep Host、Keep Local、Reject |

## 调研批次

| 批次 | 命令 | 重点 | 状态 |
|---|---|---|---|
| A | `rg`, `tree`, `gzip`, `gunzip` | Rust CLI 可链接性、是否已有 library entry | 已完成初查 |
| B | `curl`, `wget`, `tar`, `zip`, `unzip` | C/Rust 官方 CLI、进程依赖和许可证 | 已完成初查 |
| C | `sqlite3`, `git`, `which`, `clear` | 官方 shell/tool 层与 Brush-aware 需求 | 已完成初查 |
| D | `ssh`, `scp`, `sftp`, `mosh` | OpenSSH/Mosh 的进程、PTY、signal 依赖 | 已完成初查 |
| E | `vi`, `nano`, `edit` | 可嵌入编辑器及终端接口 | 已完成初查 |
| F | `python`, `python3`, `pip`, `pip3`, `node`, `ocr` | 现有专用 Host 是否已最大化复用上游 CLI | 已完成初查 |

## 调研结果

以下为第一轮架构可行性调研。`S/A` 可以直接进入实现，`B` 应先做独立
链接原型，`C` 暂不应替换当前实现。

| 命令 | 推荐上游 | 可调用入口与现状 | 许可证 | iOS/可重入阻碍 | 等级 | 决策 |
|---|---|---|---|---|---|---|
| `rg` | [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | 官方完整 CLI 位于 binary/core 层；底层 `grep-*` crates 不是完整 rg CLI | MIT / Unlicense | 需抽出 `main` 的参数、高层配置和运行路径；`--pre`、压缩搜索会启动外部程序，需 hook/禁用 | B | Prototype：vendor CLI 层，删除本地 Clap |
| `tree` | [trees-rs](https://lib.rs/crates/trees-rs) 或经典 tree | 有完整 Rust CLI，但候选项目成熟度和 API 稳定性需源码原型确认 | MIT / Apache-2.0（trees-rs） | 主要是把 binary main 抽成 `run(argv, io)`；风险相对小 | B | Prototype：先验证 flags 覆盖与可链接性 |
| `gzip` | [FreeBSD/NetBSD gzip](https://man.freebsd.org/gzip) | BSD 系提供完整 gzip CLI，基于 zlib，与 `gunzip` 共用入口 | BSD 系许可，逐文件复核 | signal、进程 stdio、全局 getopt 状态需小型补丁 | A | Adopt：优先于 GPL GNU gzip |
| `gunzip` | 同上 | 通过 argv[0] 或 `-d` 进入同一官方 CLI | 同上 | 与 gzip 共用一次 patch | A | Adopt：与 gzip 一次接入 |
| `curl` | [curl/curl](https://github.com/curl/curl) 的 tool 层 + libcurl | 官方仓库同时包含 curl CLI 与 libcurl，tool 层已有完整参数解析 | curl MIT-like | tool 层文件多；signal、全局 stdout、配置文件、部分协议/子进程能力需裁剪；二进制体积需测量 | B | Prototype：抽 `tool_main`，不要继续扩展本地 Clap |
| `wget` | [GNU Wget](https://www.gnu.org/software/wget/) / Wget2 | 有完整 C CLI | GPLv3+ | CLI 深度依赖 POSIX、signal、DNS/TLS 配置；许可证与 App 分发策略需先确认 | C | 暂缓；在采用上游前冻结本地 flags，必要时改名为受限 downloader |
| `tar` | [libarchive/bsdtar](https://github.com/libarchive/libarchive) | libarchive 自带完整 `bsdtar` CLI，公开 `main(argc, argv)` | BSD-2-Clause 为主，逐文件复核 | `lafe_errc` 退出、signal、外部压缩程序选项需要 patch/hook；核心 archive I/O 可注入 | B | Prototype：优先级高 |
| `zip` | [Info-ZIP](https://infozip.sourceforge.net/) 或 libarchive | Info-ZIP 有兼容 CLI；libarchive 可读写 ZIP，但其现成 CLI 语义是 bsdtar，不是 Info-ZIP zip | Info-ZIP BSD-like；libarchive BSD | Info-ZIP 较老且全局状态多；libarchive 若自建 zip 前端又会回到手写 flags | B | Prototype：先验证 Info-ZIP 可重入性；否则保留当前实现但冻结参数面 |
| `unzip` | 同上 | Info-ZIP 有完整 unzip CLI；libarchive 能自动识别并读取 ZIP/ZIPX | 同上 | 与 zip 类似；路径安全、密码、编码行为必须跑兼容测试 | B | Prototype：与 zip 联合评估 |
| `sqlite3` | [SQLite 官方 shell.c](https://www.sqlite.org/cli.html) | 官方明确提供单文件 CLI；源码还包含供其他项目作为子程序使用的 `sqlite3_shell` 路径 | Public Domain | 需接管退出、stdio、交互输入和少数 shell 扩展；无需重写 SQL 或 dot commands | A | **Adopt：第一优先级** |
| `git` | [Git 官方源码](https://git-scm.com/) | 官方 Git 是 multi-call CLI，但大量 builtin 仍调用其他 Git 子程序和外部工具 | GPLv2 | fork/exec、pager/editor/credential helper、hooks、全局状态极多；不是简单 `main(argv)` | C | Keep Host：继续 libgit2 子集；不要假装完整 Git CLI |
| `which` | FreeBSD/NetBSD `which` | BSD 系有很小的完整 CLI | BSD 系许可，逐文件复核 | 上游外部 `which` 不理解 Brush 函数/alias；但 `type` 已承担 Shell-aware 查询 | A | Adopt：接上游 CLI；Shell-aware 行为交给 `type` |
| `clear` | terminfo/ncurses clear，或当前实现 | 上游 clear CLI 存在，但当前实现只是少量 ANSI 序列和一个 `-x` | 当前自有 | 引入 ncurses/terminfo 的收益低于成本 | S | Keep Local：不属于“手写大量 CLI”问题 |
| `ssh` | [OpenSSH Portable](https://github.com/openssh/openssh-portable) | 官方有完整 `ssh` main | BSD/ISC 混合 | OpenSSH 深度依赖 Unix signal、PTY、termios、配置、agent/helper 和进程模型 | C | Keep Host：继续 russh；外层统一 CommandContext |
| `scp` | OpenSSH Portable | 官方有 scp CLI | BSD/ISC 混合 | 会调用 ssh/传输子系统，依赖 OpenSSH 大量内部模块；直接嵌入成本高 | C | Keep Host：继续 russh-sftp |
| `sftp` | OpenSSH Portable | 官方有 sftp CLI 和交互前端 | BSD/ISC 混合 | libedit/TTY、OpenSSH session 和配置体系较重 | C | Keep Host：继续专用 REPL |
| `mosh` | [mobile-shell/mosh](https://github.com/mobile-shell/mosh) | 官方 C++/Perl CLI | GPLv3，带 OpenSSL/iOS 相关例外；分发前法律复核 | 启动 ssh/mosh-server、PTY、ncurses、protobuf、signal；不是可直接重入的 main | C | Keep Host：继续 Rust iOS 客户端，补协议兼容测试 |
| `vi` | 当前 vendored nextvi；备选 [Vim](https://github.com/vim/vim) | Vim 有完整 main；nextvi 更小、更适合裁剪 | Vim license / nextvi 许可逐文件复核 | Vim 体积和全局状态巨大；nextvi 仍需解决重复调用、TTY 和状态重置 | B | Prototype nextvi Host；不要直接引入完整 Vim |
| `nano` | [GNU nano](https://www.nano-editor.org/git.php) | 官方完整 CLI | GPLv3+ | ncurses、termios、signal、全局编辑器状态；App 分发许可证需确认 | C | Keep Local；若追求官方兼容再单独立项 |
| `edit` | YourShell 自有编辑器模式 | 没有对应标准上游 CLI | 当前自有 | 只是同一编辑器引擎的友好入口 | S | Keep Local |
| `python3` | [CPython C API](https://docs.python.org/3/c-api/interp-lifecycle.html) | CPython 提供 `Py_Main`/`Py_BytesMain`，但当前 App 使用常驻解释器 Host | Python Software Foundation License | 初始化、扩展模块、环境和 site-packages 必须由宿主管理；重复 Main 生命周期需验证 | A | Keep Host：让 argv 尽量进入 CPython 官方解析路径 |
| `python` | 同 `python3` | 同一 Host 的别名 | 同上 | 同上 | A | Keep Host |
| `pip` | 官方 pip Python module | 当前经嵌入式 CPython driver 调 pip | MIT | iOS 禁止 pip 创建构建子进程；二进制 wheel 和 lifecycle scripts 受限 | A | Keep Host：flags 已由 pip 解析 |
| `pip3` | 同 `pip` | 同一 Host 的别名 | MIT | 同上 | A | Keep Host |
| `node` | [Node.js Mobile](https://nodejs-mobile.github.io/docs/guide/guide-ios/getting-started/) | 官方 iOS runtime 只允许单实例，结束后不能重启；当前使用常驻 Node dispatcher | Node.js 许可集合 | 不能每条命令调用一次 Node main；需要常驻线程和请求协议 | A | Keep Host：当前设计符合上游约束 |
| `ocr` | Apple Vision | 没有对应通用开源 CLI main | Apple SDK | 系统 Framework API，只需很薄的命令包装 | S | Keep Host |

## 推荐实施顺序

| 阶段 | 命令 | 目标 | 验收条件 |
|---|---|---|---|
| 0 | 通用基线 | 建立 `UpstreamMain` 审计清单和兼容测试工具 | 能比较系统 CLI 与 YourShell 的 stdout、stderr、exit code；检查 `exit/fork/exec/signal/static mut` |
| 1 | `sqlite3` | 用官方 `shell.c` 替换本地 CLI | 官方 dot commands/flags 由上游解析；重复调用不污染状态 |
| 2 | `gzip`, `gunzip`, `which` | 接入小型 BSD CLI | 删除对应本地 Clap；管道、文件、错误码兼容 |
| 3 | `tar` | 接入 libarchive/bsdtar | 常用 GNU/BSD tar flags 有明确兼容结果；外部 compressor 走进程内 hook 或明确报错 |
| 4 | `rg` | 抽取官方 ripgrep CLI 层 | 删除 `RipgrepCommand` 本地 flags；官方 help 与参数覆盖生效 |
| 5 | `zip`, `unzip`, `tree` | 完成候选原型后择优 | 不新增本地 flag schema；兼容测试达到约定阈值 |
| 6 | `curl` | 接入官方 tool 层原型 | 先限定 HTTP(S)/FILE；官方 parser 生效；评估体积和 TLS |
| 7 | `wget`, `git`, OpenSSH、编辑器 | 依据许可证和原型结果再决策 | 没有低风险上游入口时，不为“形式统一”牺牲稳定性 |

## a-Shell / ios_system 对照

[a-Shell](https://github.com/holzschu/a-shell) 是与 YourShell 最接近的公开
iOS 工程。它没有为所有命令重新实现 flags，而是使用两种后端：

```text
NativeMainBackend
    command name
        ↓
    framework + symbol
        ↓
    command_main(argc, argv)

WasiBackend
    command name
        ↓
    预编译 WebAssembly CLI
        ↓
    WASI argv/env/files/stdin/stdout/stderr
```

`ios_system` 的命令字典直接记录：

```text
命令名 → framework → 函数符号 → getopt 元数据 → 参数类型
```

例如：

```text
curl   → curl_ios.framework → curl_main
gzip   → files.framework    → gzip_main
gunzip → files.framework    → gzip_main
tar    → tar.framework      → tar_main
ssh    → ssh_cmd.framework  → ssh_main
```

### 26 个命令与 a-Shell 的对应关系

| YourShell 命令 | a-Shell 状态 | a-Shell 接入方式 | 对 YourShell 的启示 |
|---|---|---|---|
| `rg` | 可选包 | 官方 CLI 预编译为 WASM | 优先研究 WASI Backend；可避免本地 Clap 和大量 iOS patch |
| `tree` | 未在当前包清单发现 | — | 仍需独立选择上游；适合作为 WASI 小型验证命令 |
| `gzip` | 内置 | BSD gzip `gzip_main(argc, argv)`，native framework | 可直接参考 ios_system 的 BSD 移植，而不是从头 patch |
| `gunzip` | 内置 | 与 gzip 共用 `gzip_main`，由 argv[0] 决定行为 | 完全符合 multicall/uumain 模式 |
| `curl` | 内置 | `curl_ios.framework` → `curl_main` | 直接参考其 curl tool 层 iOS patch 和构建配置 |
| `wget` | 未发现 | — | a-Shell 也选择 curl 而非同时维护 wget；可考虑暂不提供 wget 兼容名 |
| `tar` | 内置 | `tar.framework` → `tar_main`，BSD 来源 | 直接研究并复用 ios_system tar port |
| `zip` | 可选包 | Info-ZIP 系 CLI 预编译为 WASM | WASI 比 native 全局状态 patch 更合适 |
| `unzip` | 随 zip 包安装 | `unzip.wasm3`，与 zip 工具族一起发布 | 以整个上游工具族接入，不单独手写 unzip |
| `sqlite3` | 可选包 | 官方 sqlite3 CLI 预编译为 WASM | 两条可行路线：官方 `sqlite3_shell` native，或复用 WASI 路线 |
| `git` | 不提供官方 Git | `git` 只是 `lg2` 包装器，并明确提示不完全兼容 | 验证了官方 Git main 不适合 iOS；不要宣称完整兼容 |
| `which` | 可选包 | 预编译命令包 | 可参考其上游来源；或 native 接入小型 BSD which |
| `clear` | App/Shell 内部能力 | 未作为大型外部 CLI framework | 保留本地 ANSI 实现即可 |
| `ssh` | 内置 | `ssh_cmd.framework` → `ssh_main` | 可研究 a-Shell/OpenSSH/libssh2 port，但当前 russh 仍可能更易并发 |
| `scp` | 内置 | `ssh_cmd.framework` → `scp_main` | 可借鉴 argv/配置兼容，不必立即替换 russh-sftp |
| `sftp` | 内置 | `ssh_cmd.framework` → `sftp_main` | 可借鉴完整 CLI parser 和命令集 |
| `mosh` | 未发现 | — | a-Shell 没提供参考；继续现有 Rust 实现 |
| `vi` | a-Shell 提供 Vim | 单独的 iOS Vim 集成，不是普通小型 command main | 需要单独研究 iVim/a-Shell 终端桥，不能按普通 CLI 估算 |
| `nano` | 未发现 | — | a-Shell 也没有选择 GNU nano |
| `edit` | 有 Vim、Ed、Kilo 等替代 | native Ed / 可选 Kilo | 可评估小型开源编辑器，不必绑定 nano 兼容 |
| `python3` | 内置 | Python framework → `python_main` | 与当前 Python Host 同方向，可对照 argv 和环境处理 |
| `python` | 内置 | Python framework → `python_main` | 同上 |
| `pip` | 由 Python 环境提供 | 嵌入 CPython 内执行 | 当前 Host 方向正确 |
| `pip3` | 由 Python 环境提供 | 嵌入 CPython 内执行 | 当前 Host 方向正确 |
| `node` | 没有对应常规 shell 命令 | — | 当前 NodeMobile 常驻 Host 是独立选择 |
| `ocr` | 未发现 | — | 保留 Apple Vision Host |

### ios_system 的移植规则

`ios_system` 官方 README 给出的典型接入步骤是：

1. 优先寻找 BSD 许可的命令源码。
2. 将上游 `main()` 改名为 `command_main()`。
3. 用 `ios_error.h`/宿主替换层拦截 `exit`、`warn`、`err`、`printf`、
   `write` 等调用。
4. 将 `isatty()` 改成 Session-aware 版本。
5. 将 stdin/stdout/stderr 转到当前命令线程的流。
6. 初始化并释放每次运行状态。
7. 将进程全局变量改成 thread-local。
8. 审计并替换 `fork`、`exec`、`system`、`popen`、`access`。
9. 多次运行测试，检查第二次及并发调用的状态泄漏。

这与 YourShell 当前 `command_host` 的最大区别是：

```text
YourShell 当前：
    临时替换进程 cwd/env/fd
    → 全局锁

ios_system：
    thread-local stdin/stdout/stderr + libc 替换层
    → 允许命令按线程区分上下文
```

因此 a-Shell 的价值不仅是提供已移植命令源码，还提供了消除全局锁的一种
经过 App Store 产品验证的参考架构。

### 建议新增两个统一 Backend

```text
NativeMainBackend
├── name
├── fn(argc, argv) -> exit_code
├── thread-local SessionContext
└── libc/exit/system 替换层

WasiBackend
├── .wasm module
├── argv/env
├── preopened Session cwd
├── stdin/stdout/stderr
└── exit code
```

优先验证：

| 原型 | 参考 | 要验证的问题 |
|---|---|---|
| Native `gzip` | ios_system `files.framework/gzip_main` | 能否直接复用 BSD port；Session 间并发是否不再需要全局锁 |
| Native `curl` | ios_system `curl_ios.framework/curl_main` | 官方 parser、TLS、stdout/stderr 和重复调用 |
| Native `tar` | ios_system `tar.framework/tar_main` | 外部压缩命令 hook、文件权限和路径安全 |
| WASI `rg` | a-Shell-commands `rg` | Rust WASI 模块体积、速度、`.gitignore`、管道 |
| WASI `sqlite3` | a-Shell-commands `sqlite3` | 交互输入、数据库文件映射、退出码 |
| WASI `zip/unzip` | a-Shell-commands `zip` 工具族 | 文件权限、编码、Zip64 和性能 |

## 关键官方依据

- [uutils/coreutils](https://github.com/uutils/coreutils)：以 GNU 行为兼容为目标的跨平台 Rust coreutils。
- [ripgrep 官方仓库](https://github.com/BurntSushi/ripgrep)：完整 CLI 与底层搜索 crates 同仓库，但高层 CLI 需要抽取。
- [libarchive 官方仓库](https://github.com/libarchive/libarchive)：包含完整 bsdtar，并支持 tar、ZIP/ZIPX 等格式。
- [curl 官方仓库](https://github.com/curl/curl)：同仓库提供 curl tool 层和 libcurl，采用 MIT-like 许可。
- [SQLite 官方 CLI 文档](https://www.sqlite.org/cli.html)：官方 CLI 由 `shell.c` 与 SQLite 引擎共同构建。
- [SQLite 许可说明](https://github.com/sqlite/sqlite/blob/master/LICENSE.md)：SQLite 引擎与 CLI 构建代码属于 Public Domain。
- [OpenSSH Portable](https://github.com/openssh/openssh-portable)：官方可移植 OpenSSH，但仍是完整 Unix 应用架构。
- [Mosh 官方仓库](https://github.com/mobile-shell/mosh)：官方实现依赖 C++、protobuf、终端和客户端启动层。
- [CPython 初始化与 Main API](https://docs.python.org/3/c-api/interp-lifecycle.html)：提供 `Py_Main`/`Py_BytesMain`，嵌入场景仍需宿主管理 runtime 生命周期。
- [Node.js Mobile iOS 指南](https://nodejs-mobile.github.io/docs/guide/guide-ios/getting-started/)：单个 App 只能启动一个 Node runtime，且结束后不支持重启。
- [a-Shell 官方仓库](https://github.com/holzschu/a-shell)：公开的 iOS 多窗口终端，使用 ios_system 和 WASM 命令。
- [ios_system 官方 README](https://github.com/holzschu/ios_system)：记录 `command_main(argc, argv)`、函数注册、thread-local I/O 和 iOS 移植规则。
- [ios_system commandDictionary](https://github.com/holzschu/ios_system/blob/master/Resources/commandDictionary.plist)：命令到 framework/function 的实际映射。
- [a-Shell-commands 包清单](https://github.com/holzschu/a-Shell-commands)：提供 `rg`、`sqlite3`、`zip/unzip` 等 WASM CLI。

## 下一轮原型前需要确认

1. 项目最终分发许可证策略，尤其是否接受 GPLv3 组件。
2. App 体积预算，决定是否能引入 libcurl tool、libarchive 和完整编辑器。
3. 兼容目标是 GNU、BSD、官方 upstream 还是“明确子集”。
4. 每个候选上游的固定版本、更新节奏和 patch 维护方式。
5. Session 级 I/O 注入完成前，哪些命令允许继续使用全局
   `process_state_lock`。

## 当前实现复盘

### 执行模型

```text
iOS / Swift
    │
    ├── Session A：OS thread + current-thread Tokio + Brush Shell
    └── Session B：OS thread + current-thread Tokio + Brush Shell
                                      │
                         Brush parse / expand / pipeline
                                      │
              ┌───────────────────────┼────────────────────────┐
              │                       │                        │
       ShellBuiltin             ProcessMain              NativeHost
       Brush 内建语义           command_host             直接使用 Session fd
       修改当前 Shell           全局状态桥               或常驻 runtime
              │                       │                        │
      cd/export/read...      uutils/grep/sed/...      ssh/sftp/mosh/node...
```

每个 Session 确实拥有独立的 Brush `Shell`、cwd、env、fd table，并且 Session
之间有独立 OS 线程。这里的隔离是正确的。

但“上游程序认为自己拥有整个进程”，与“多个 Shell Session 共用一个 App
进程”存在冲突。`command_host::dispatch` 当前通过以下步骤弥合它：

1. 从 Brush Session 取出 argv、cwd、导出的 env、fd 0/1/2。
2. 非交互 stdin 在拿锁前全部读完，避免 pipeline 上下游互相等锁。
3. `spawn_blocking` 运行同步上游入口。
4. 获取全局 `process_state_lock`。
5. 临时 `chdir`、`setenv`、`dup2`。
6. 调用统一的 `fn(&CmdCtx) -> i32`。
7. 恢复 fd、env、cwd，并重新设置 `SIGPIPE -> SIG_IGN`。

这个设计首先保证了正确性，且已经消除了各 adapter 重复实现保存/恢复逻辑；
作为过渡层是合理的。

### 当前三类接入的评价

| 接入类 | 当前例子 | 优点 | 主要问题 | 结论 |
|---|---|---|---|---|
| Brush Shell builtin | 61 个 Bash builtins | 能直接修改当前 Shell；语义完整；不碰进程全局状态 | 与普通 CLI 不同，不能共用 `main(argv)` | 必须保留 |
| ProcessMain adapter | 74 个 uutils，以及 grep/sed/awk/jq/find 等 | 上游负责 flags、help、错误码；YourShell 代码很薄 | cwd/env/fd 是进程全局，跨 Session 被一把锁串行 | 保留为兼容后端，不应成为最终唯一后端 |
| Native/Resident Host | ssh/sftp/mosh、Node、编辑器、OCR、git 等 | 可直接使用 Session fd；能承载异步、交互或常驻 runtime | 每类 Host 形状不同；注册与能力信息分散 | 保留，但统一注册契约 |

### 当前实现做得好的地方

- Brush 只负责 Shell 语法与 Shell 状态，成熟 CLI 的 flags 大量交给上游。
- uutils 是正确的 multicall 模式：一次注册逻辑覆盖约 74 个命令。
- `command_host` 集中处理危险的进程状态保存与恢复，没有继续复制 adapter。
- SSH/SFTP/Mosh 已绕开全局 stdio 锁，证明项目已经具备 Session-local I/O
  的可行路径。
- Python 和 Node 按 runtime 生命周期做专用 Host，没有机械地每次调用 `main`。
- redirected stdin 在加锁前 drain，避免了同一 pipeline 的典型锁死。
- 对 stale uutils exit code、vendored library 修改 SIGPIPE 等真实重入问题已有防护。

### 当前实现的主要结构问题

| 问题 | 影响 |
|---|---|
| `build_shell()` 手工串联所有 `.builtin(...)` | 命令名、来源、feature、backend、alias、并发能力无法统一查询和验证 |
| `Registration` 只描述 Brush builtin，不描述执行能力 | 看不出命令是否需要全局锁、是否交互、是否支持取消、是否可并发 |
| `CommandMain = fn(&CmdCtx) -> i32` 仍假设上游读进程 stdio/cwd/env | 函数签名看似注入了 context，实际上多数上游并未消费这些字段 |
| `process_state_lock` 覆盖 cwd、env、fd 三类状态 | 即使某命令只需要其中一种，也被最强串行策略约束 |
| timeout/cancel 只能停止等待，不能停止 `spawn_blocking` 中的命令 | 后台命令可能继续持有 fd 或全局锁 |
| redirected stdin 总是完全缓冲到临时文件 | 大流量 pipeline 增加内存和延迟，无法真正流式执行 |
| 上游版本、补丁、许可证、兼容目标没有成为注册数据 | 更新和审计依赖人工记忆 |
| 本地 Clap 命令与上游 adapter 混在 `build_shell()` | 难以设置“禁止新增手写 flags”的工程规则 |

## 关于 a-Shell 方案的边界

a-Shell/ios_system 很值得参考，但不能把结论简化成“thread-local 后就没有锁”。

| 状态 | thread-local/libc wrapper 能否解决 | 限制 |
|---|---|---|
| stdin/stdout/stderr | 通常可以 | 前提是所有相关 `read/write/stdio` 都经过可拦截层 |
| `isatty`、`exit`、`err/warn` | 可以通过 port patch | 每个 C 上游都要审计 |
| C 的自有全局变量 | 可改成 TLS 或每次 reset | 需要维护上游 patch |
| cwd | 不能被普通 TLS 自动虚拟化 | `open("relative")` 仍使用真实进程 cwd，除非拦截全部路径调用或改上游 |
| environment | 不能被普通 TLS 自动虚拟化 | `getenv/environ` 需要完整 wrapper；Rust `std::env` 不一定经过可替换符号 |
| Rust `std::fs` / `std::env` | 不能假设可被 ios_system 拦截 | uutils/ripgrep 仍可能要求锁或源码级 context 注入 |
| fork/exec/system/popen | 必须改写 | 可映射到内部 command registry，但进程语义无法完全复制 |

因此 Native Main backend 只能声明“它实际隔离了哪些状态”，不能笼统标为
thread-safe。WASI 的价值正是把 argv/env/cwd/preopen/fd 放进每个实例，天然
更接近 Session 模型。

## 建议的目标架构

不要把 171 个名字都强制转成同一个实现；应统一的是**注册和调用契约**，不是
所有命令的内部运行机制。

```text
CommandRegistry
    │
    ├── ShellBuiltinBackend
    │       Brush Registration
    │
    ├── InjectedBackend
    │       run(CommandContext) -> Future<Exit>
    │
    ├── WasiBackend
    │       module + WASI Session view
    │
    ├── NativeMainBackend
    │       upstream main + audited process capabilities
    │
    └── ResidentRuntimeBackend
            Node / Python / editor / protocol host
```

建议统一的元数据：

```text
CommandSpec {
    names / aliases
    backend
    upstream + version + license
    feature gate
    state semantics
    io mode
    isolation: SessionLocal | StdioTls | ProcessGlobal
    cancellation: Cooperative | HostAbort | None
    compatibility target
}
```

其中优先级应是：

1. `ShellBuiltinBackend`：只给必须参与 Shell 状态的命令。
2. `InjectedBackend`：最佳形态，上游直接接受 argv/cwd/env/I/O，不碰全局状态。
3. `WasiBackend`：适合文件型、计算型、无 socket/fork 的完整 CLI。
4. `NativeMainBackend`：兼容兜底；按实际使用的进程状态决定锁粒度。
5. `ResidentRuntimeBackend`：给 Node、Python、交互编辑器、协议客户端。

### 不建议做一个通用 `NativeMain` 就结束

只做 `name -> main(argv)` 能统一表面代码，却不能解决：

- 多 Session cwd/env 隔离；
- streaming pipeline；
- cancellation；
- 上游 `exit()` 杀死 App；
- signal/global/static 状态泄漏；
- iOS 不允许的 fork/exec/JIT。

uutils 模式成功，是因为它同时满足“上游已有可调用入口”和“当前 Host 做了
完整进程状态桥”。它不是所有命令都能无成本复制的证据。

## 改进路线（只规划，尚未实施）

| 阶段 | 改动 | 目的 | 风险 |
|---|---|---|---|
| 1 | 把现有注册整理为静态 `CommandSpec` 清单 | 171 个名字可自动盘点；消除 `build_shell()` 手工遗漏 | 低 |
| 2 | 给 backend 标注 isolation/cancel/io 能力 | 测试和调度能知道何时必须拿全局锁 | 低 |
| 3 | 建立 upstream compatibility harness | 比较 help、stdout、stderr、exit、重复调用、并发调用 | 低 |
| 4 | 将已直接使用 Session fd 的 SSH/SFTP/Mosh 抽成 `InjectedBackend` 契约 | 先统一成熟的无锁路径 | 中 |
| 5 | 做一个 Native Main 小原型：BSD gzip | 验证 ios_system patch、TLS stdio、cwd/env 仍需何种锁 | 中 |
| 6 | 做一个 WASI 小原型：`rg` 或 `sqlite3` | 验证 iOS runtime、体积、性能、preopen、管道、取消 | 中 |
| 7 | 根据原型结果逐命令迁移 | 删除本地 Clap，而不是先批量重构全部命令 | 中 |
| 8 | 最后按能力拆小或移除 `process_state_lock` | 实现 Session 内串行、Session 间尽可能并发 | 高 |

### 第一批应删除本地 flag schema 的命令

| 优先级 | 命令 | 推荐路线 |
|---|---|---|
| P0 | `sqlite3` | 优先 WASI；备选官方 `sqlite3_shell` Native Main |
| P0 | `gzip`, `gunzip` | ios_system/NetBSD Native Main 原型 |
| P0 | `rg` | 官方 CLI 的 WASI 构建 |
| P1 | `tar` | ios_system BSD tar Native Main |
| P1 | `zip`, `unzip` | Info-ZIP WASI 工具族 |
| P1 | `curl` | ios_system curl tool Native Main，保留网络 Host 能力审计 |
| P2 | `tree`, `which` | 小型上游 Native Main 或 WASI |
| 暂缓 | `wget` | 与 curl 功能重叠，先冻结扩展 |
| 不机械替换 | `git`, OpenSSH/Mosh、编辑器、Python/Node/OCR | 继续专用 Host，统一注册契约即可 |

## 结论

当前架构不是“171 个命令都手写了”，而是：

- 61 个必须 Shell-native；
- 84 个已经把主要 CLI 解析交给上游；
- 其余命令中，一部分本地 Clap 应被上游 Native/WASI CLI 替换；
- 另一部分天然需要专用 runtime 或协议 Host。

最值得改的不是先增加更多 adapter，而是先建立统一 `CommandRegistry +
Backend capabilities`，再用 Native Main 与 WASI 两个原型证明隔离、兼容、
体积和可维护性。只有原型通过后，才应逐个删除本地 Clap；不应一次性把全部
命令强行塞进 `uutils_adapter` 的进程全局模型。
