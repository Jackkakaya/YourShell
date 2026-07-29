# 命令上游调研结论与实施计划

状态：完成源码/官方资料层面的第一轮调研，尚未修改命令实现。

本计划替代此前未经充分验证的优先级。实施前仍需做目标平台编译原型，因为
“上游有 main”不等于“能在当前 Rust staticlib + iOS App 中无修改链接”。

## 1. 已确认的现有基础

YourShell 已经具备 Native CLI 接入所需的基本设施：

- `core/build.rs` 已用 `cc::Build` 编译并静态链接 One True Awk 和 jq C CLI；
- `command_host` 已能把 Session argv/cwd/env/fd 映射给 process-shaped main；
- iOS device 与 simulator Rust targets 已安装；
- uutils、grep、sed、findutils、diffutils 已证明 Rust `uumain(argv)` 模式；
- awk、jq 已证明 C `main(argc, argv)` 改名后可在当前工程重复调用。

因此默认方案仍是扩展现有 Native 模式，不新增 runtime。

## 2. 调研结论总表

### 2.1 已有但实现不够好的命令

| 命令 | 上游调研结果 | 入口/障碍 | 许可证 | 决策 |
|---|---|---|---|---|
| `sqlite3` | 官方 CLI 全部位于单个 `shell.c`，与 `sqlite3.c` 构建 | main 可改名；需处理 exit、交互状态和重复调用 | Public Domain | **首批 Native 原型** |
| `gzip/gunzip` | ios_system 已在 iOS 使用 BSD gzip，同一 `gzip_main` 按 argv[0] 分派 | getopt/global/stdio patch 已有参考 | BSD | **首批 Native 原型** |
| `tar` | ios_system 有 BSD tar；当前 libarchive 也提供完整 bsdtar | bsdtar main 清晰，但使用 signal、`lafe_errc`，libarchive 部分遍历会 chdir，解包还涉及 process umask | BSD 类 | **优先 libarchive/bsdtar Native 原型** |
| `curl` | a-Shell/ios_system 已使用 `curl_main`；官方仓库同时提供 tool 与 libcurl | tool 层源文件较多；需审计 signal、配置文件、TLS、全局状态和体积 | curl license，宽松 | **Native 原型，首批但独立评估体积** |
| `rg` | 官方完整 parser 位于 ripgrep binary/core 高层，不是已发布的稳定 library API | 需要 vendor CLI 高层并抽 entry；`--pre` 和压缩搜索会找外部程序 | MIT/Unlicense | **Native 可行，但不是低成本首批** |
| `tree` | 官方 Steve Baker tree 有完整 main，但为 GPLv2+，且源码有大量 globals | 可 Native 移植，但重复调用需要系统性 reset/TLS；许可证需先决定 | GPLv2+ | **暂缓，先做许可证决策和替代候选比较** |
| `unzip` | libarchive 官方提供 `bsdunzip` 完整 CLI | 已接入并通过重复调用/路径行为测试；语义目标是 bsdunzip | BSD 类 | **已完成** |
| `zip` | libarchive 能创建 ZIP，但没有 Info-ZIP 语义的 `zip` CLI | Info-ZIP 上游老、globals 多；iOS 移植成本高 | Info-ZIP license | **保留 Rust `zip` crate，增量增强** |
| `wget` | a-Shell 没有 Native Wget；GNU Wget 是完整成熟 CLI | 网络/TLS/config/递归下载依赖面大 | GPL | **先做产品与许可证决策，不进入首批实现** |

### 2.2 当前缺失的基础命令

| 命令 | 上游调研结果 | 决策 |
|---|---|---|
| `stat` | uutils 当前主线已有 `uu_stat`，MIT；属于 Unix feature set。我们 vendored 的 `brush-coreutils-builtins` 0.8 注册集合没有它 | **优先扩展现有 uutils registry，不引入 BSD C** |
| `egrep/fgrep` | ios_system 也只是让两个名字进入同一 grep main；现有 `uu_grep` 已有 `-E/-F` | **在 grep adapter 做 argv[0] 分派/参数注入** |
| `pbcopy/pbpaste` | a-Shell 为 App 内部命令，不需要第三方 CLI | **Swift Clipboard Host** |
| `open/openurl` | a-Shell 为 App/Shell 集成能力 | **Swift UIApplication Host** |
| `bc/dc` | ios_system dictionary 显示两个名字共享 `bcdc_main`；存在 iOS Native 产品实证 | **先定位实际 bc_ios 源和逐文件许可证，再做单一 C adapter 原型** |
| `dig/host/nslookup/nc/ping/whois` | `network_ios` 官方明确提供这些命令、BSD-3-Clause，并有 xcframework 构建 | **作为一个工具族做链接原型；不是六套自建 parser** |
| `ssh-keygen` | ios_system/a-Shell 有 OpenSSH port 的 keygen main，但当前 YourShell SSH 是 russh | **只评估 keygen 所需模块；禁止顺带引入第二套 SSH session runtime** |
| `cal/ncal` | a-Shell 可选包提供；适合 BSD CLI 工具族 | **第二批候选，先固定 BSD 上游版本** |
| `hexdump` | 不属于 GNU coreutils/uutils 当前命令集，不能通过现有 74 个 registry 自动获得 | **选 BSD hexdump Native main，不手写 format parser** |
| `strings` | 可选 LLVM/BSD 实现；LLVM 全量依赖过重 | **优先 BSD 小型实现** |
| `getopt` | 与 Shell builtin `getopts` 不同；完整兼容一般来自 util-linux | **许可证和依赖面先评估，暂不首批** |
| `column` | 通常来自 util-linux/BSD，实现涉及 Unicode width 与多模式 parser | **选择 BSD/宽松许可实现后再排期** |
| `xz/lzmadec` | XZ Utils 官方提供完整 CLI；核心组件新版本为 0BSD，但仓库中其他文件许可证混合 | **逐文件选取 CLI/core 后做 Native 原型** |
| `say` | 本质是 Apple 平台能力 | **Swift AVSpeechSynthesizer Host** |
| `uptime` | uutils uptime 位于 utmp/utmpx feature set，不适合直接假设 iOS 可用；ios_system 有 BSD port | **先定义 iOS 可承诺的输出语义，再决定是否提供** |
| `chflags` | ios_system 有 BSD Native 实现 | **先验证 iOS 文件系统实际支持的 flags** |
| `ed` | ios_system 有 BSD Native 实现 | **交互和重复调用专项原型，非首批** |

## 3. 关键修正

与之前清单相比，本轮调研得出以下修正：

1. `stat` 首选 uutils，不首选 BSD C。
2. `tar` 首选当前活跃的 libarchive/bsdtar 做原型，ios_system port 作为 iOS
   patch 参考。
3. `unzip` 可以跟随 libarchive；`zip` 不能因此自动解决。
4. 官方 `tree` 不是宽松许可的小型 BSD 工具，而是 GPL 且有较多全局状态。
5. Wget 没有 a-Shell Native 成功案例，且 GNU 上游是 GPL，不能直接排入开发。
6. `network_ios` 是真实可用的 Native 工具族候选，但要验证静态链接、取消和
   与 `command_host` 的组合，不能照搬 xcframework 后就算完成。
7. uutils 主线已经比当前 vendored 0.8 registry 覆盖更多命令；补命令前应先
   检查“升级/扩展 uutils”而不是另找实现。

## 4. 实施前准入规则

每个上游必须先通过一个最小可执行原型，才能修改正式注册：

| Gate | 必须证明 |
|---|---|
| Source | 固定 repo、tag/commit、逐文件许可证和 NOTICE |
| Build | macOS host、iOS simulator、iOS device 三个目标可编译链接 |
| Entry | 入口接受调用方 argv，不能读取宿主进程原始启动 argv |
| Exit | bad flag、`--help`、运行错误均不会调用不可拦截的 process exit |
| Repeat | success→success、failure→success 至少三轮无 stale state |
| I/O | stdin、stdout、stderr、重定向和 pipeline 均经过 Brush fd |
| Files | 相对路径按 Session cwd，错误后 cwd/env/fd 完整恢复 |
| Cancel | 长命令的当前限制有明确说明；不能制造永久持锁 |
| Compat | 与固定上游的 help、关键 flags、stdout/stderr、exit code 对比 |
| Size | 记录 device release staticlib/App 增量 |

任一 Gate 失败时先记录原因，不转而手写其 flags。

## 5. 实施 Plan

### Phase 0：基线和规则

1. 建立 `upstream-manifest`：命令、上游版本、commit、许可证、patch 列表。
2. 扩展现有 flag coverage 测试为 upstream differential harness。
3. 增加重复调用序列：成功→成功、失败→成功、两个 Session 交错。
4. 固化规则：成熟 CLI 禁止新增本地 Clap fields。
5. 清除未注册的旧 `JqCommand`，避免重复实现继续被误用。（已完成）

完成标准：不改用户可见命令行为，但后续每个 upstream 都能用同一套 Gate
验收。

### Phase 1：验证最小风险的现有模式

#### 1A. `stat`

1. 在当前 uutils 版本线确认 `uu_stat` 的 iOS 编译条件。
2. 扩展 `brush-coreutils-builtins` feature/registry。
3. 复用现有 `uutils_adapter`，不新建专用 parser。
4. 对比 GNU/uutils stat 的 format flags、symlink、stdin fd 行为。

这是整个计划的第一个任务，因为它能验证“优先扩展既有上游工具族”。

#### 1B. `egrep/fgrep`

1. 为现有 grep adapter 增加命令名。
2. `egrep` 在 argv 中注入 `-E`，`fgrep` 注入 `-F`。
3. 其余参数仍完全交给 `uu_grep`。

#### 1C. App Host

分两组：

- `pbcopy/pbpaste`：Swift clipboard bridge；
- `open/openurl`：Swift URL/file opening bridge。

这些命令没有成熟通用 CLI 需要移植，Host 才是最小实现。

### Phase 2：用三个原型验证 C Native 路线

三个原型只进入隔离分支/feature，不立即替换正式命令：

#### 2A. SQLite shell

- vendor 固定版 `shell.c` 和所需生成物；
- 暴露 `ys_sqlite_main(argc, argv)`；
- 处理 exit、stdio、交互状态与重复调用；
- 与当前 `rusqlite` 实现并行跑差分测试；
- Gate 全过后替换本地 `SqliteCommand`。

#### 2B. BSD gzip

- 从 ios_system 实际使用的 BSD port 追溯原始上游与 patch；
- `gzip/gunzip` 共用一个入口；
- 验证 stdin streaming、metadata、suffix、recursive、test/list；
- 通过后删除本地 GzipCommand/GunzipCommand。

#### 2C. libarchive

- 先只编译 libarchive + bsdtar + bsdunzip；
- 禁用/拦截外部 compressor program；
- 审计 `lafe_errc`、signal、chdir、umask；
- 分别验证 tar 和 unzip，不把 zip 纳入成功条件；
- 通过后替换本地 TarCommand 和 UnzipCommand。

Phase 2 的目标是验证现有 `command_host` 对三种不同 C CLI 的通用性，而不是
增加新的 backend。

### Phase 3：网络 CLI

#### 3A. curl

- 以官方 curl tool + libcurl 为准，a-Shell patch 仅作 iOS 参考；
- 先限定官方构建 feature/protocol 集，不修改 parser；
- 测量 TLS、CA bundle、config file、重复调用和 App 体积；
- 通过后替换本地 ureq CurlCommand。

#### 3B. network_ios 工具族

- 在隔离 feature 下链接 `network_ios` 源或静态产物；
- 一次注册 `dig/host/nslookup/nc/ping/whois`；
- 第一轮优先放行 DNS 三件套与 whois；
- `nc/ping` 额外验证交互、长连接、取消和 iOS 权限后再启用。

### Phase 4：高 patch 成本命令

#### 4A. ripgrep

- 固定官方版本；
- vendor `crates/core` CLI 高层而不是继续复制 flags；
- 抽 argv entry；
- `--pre`、压缩搜索的外部命令行为通过 Brush exec hook 或明确 unsupported；
- 差分覆盖达到约定阈值后替换现有 616 行 RipgrepCommand。

#### 4B. curl 之外的 archive 工具

- `zip` 单独评估 Info-ZIP Native port；
- 若 globals/reset 成本过高，保留现有明确 subset 也优于假装已由 libarchive
  完整解决；
- `xz/lzmadec` 采用 XZ Utils 固定版本和逐文件许可证清单。

#### 4C. SSH key management

- 只编译 OpenSSH keygen 所需模块做 size/link 原型；
- 输出格式必须与现有 russh 能读取的 OpenSSH key 格式互通；
- 不替换当前 ssh/scp/sftp runtime。

### Phase 5：需要产品决策的命令

以下项目在决策前不开发：

| 命令 | 必须先决定 |
|---|---|
| `tree` | 是否接受 GPLv2+；若不接受，选择哪个兼容目标 |
| `wget` | 是否接受 GPL；是否真的需要完整 Wget，还是取消该兼容名 |
| `vi/nano` | 保留兼容别名、改名，还是引入真正编辑器 |
| `git` | 产品承诺是 libgit2 subset 还是完整 Git |
| `uptime` | iOS 上可展示哪些稳定且不误导的信息 |
| `column/getopt` | BSD 还是 GNU/util-linux 兼容目标 |

## 6. 暂不进入 Plan

以下命令不进入近期实施：

- `setenv/unsetenv`：Bash 已有 `export/unset`；
- `compress/uncompress`：等待 `.Z` 真实需求；
- `ifconfig`：iOS 无完整配置权限；
- `rlogin/telnet`：明文旧协议；
- `wol`：需求较窄；
- LLVM、Jupyter、Pandoc、TeX、Web 编辑器：属于独立产品能力。

## 7. 推荐实际执行顺序

```text
0. 测试基线 + upstream manifest
1. stat
2. egrep / fgrep
3. pbcopy / pbpaste / open / openurl
4. SQLite shell 原型
5. BSD gzip 原型
6. libarchive bsdtar + bsdunzip 原型
7. curl tool 原型
8. network_ios DNS 工具族原型
9. ripgrep CLI 高层原型
10. zip / xz / ssh-keygen 独立评估
11. tree / wget / git / 编辑器产品决策
```

前三步验证低风险路径，4–8 验证 Native C CLI，9 以后才处理高 patch 或高产品
决策成本项目。

## 8. 主要官方依据

- [ios_system README](https://github.com/holzschu/ios_system)：Native
  `command_main` 移植规则、replaceCommand、ios_execv、thread-local I/O、
  内置命令和许可证说明。
- [ios_system command dictionary](https://github.com/holzschu/ios_system/blob/master/Resources/commandDictionary.plist)：
  实际命令到 `gzip_main`、`tar_main`、`curl_main` 等符号的映射。
- [network_ios](https://github.com/holzschu/network_ios)：明确列出
  ping、nc、nslookup、host、dig、telnet、whois，BSD-3-Clause。
- [uutils/coreutils](https://github.com/uutils/coreutils)：MIT，当前主线包含
  Unix `stat`，每个 utility 是独立 `uu_*` package。
- [SQLite CLI](https://www.sqlite.org/cli.html)：官方 CLI 位于单个 `shell.c`。
- [libarchive](https://github.com/libarchive/libarchive)：官方提供 bsdtar 和
  bsdunzip；同时记录 chdir/umask 等线程安全边界。
- [curl](https://github.com/curl/curl)：官方 tool 与 libcurl。
- [ripgrep](https://github.com/BurntSushi/ripgrep)：MIT/Unlicense；完整 CLI
  高层与底层 crates 同仓库。
- [Steve Baker tree](https://gitlab.com/OldManProgrammer/unix-tree)：官方 tree
  CLI，GPLv2+，源码包含较多进程全局状态。
- [GNU Wget](https://www.gnu.org/software/wget/)：官方完整 CLI，GPL。
- [XZ Utils](https://tukaani.org/xz/)：完整 xz CLI；核心组件新版本 0BSD，
  但需逐文件核对混合许可证。
