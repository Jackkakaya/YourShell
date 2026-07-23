# YourShell 设计文档

> v0.1 · 2026-07-24 · 状态：调研完成，架构定稿待评审

## 1. 定位

YourShell 是一个 iOS 原生终端 App：**Rust 核心（brush-core）+ Swift 壳**，在 iOS 的三大限制（无 fork/exec、无 JIT、沙盒签名）下提供接近桌面的 shell 体验。

目标能力（按优先级）：
1. 完整的 bash 兼容 shell（语法、管道、重定向、多窗口会话隔离）
2. 常见 Linux 命令（对齐 a-Shell 的 263 命令清单，逐步超越）
3. **python3 + pip**（官方 CPython iOS 版）
4. **node + npm**（nodejs-mobile）
5. wasm 运行时作为第三方命令分发通道（`pkg install` 生态）

与 a-Shell 的差异化：a-Shell 是 2018 年技术条件下的 C 生态组装（ios_system + 手工 patch 的几十个 C 库 + WebView 终端）；YourShell 押注 2026 年的新基建——brush-core 的实例级隔离、CPython 官方 iOS 支持、Rust CLI 生态、wasi 分发——用十分之一的胶水代码达到同等能力，且核心可测试（当前 102 用例全绿）。

## 2. 已验证的基础（截至本文档）

| 事项 | 状态 |
|---|---|
| brush-core 交叉编译 iOS（真机+模拟器） | ✅ 3 处补丁，已提上游 PR [reubeno/brush#1246](https://github.com/reubeno/brush/pull/1246) |
| 单进程多会话（实例级 cwd/env/fd 表） | ✅ brush-core 原生支持，无需改造 |
| 进程内命令注册（builtin 机制 + 真管道） | ✅ 9 个命令 + 102 用例在模拟器全绿 |
| FFI 会话层（Rust 线程 + tokio + 管道泵） | ✅ `core/src/lib.rs`，约 300 行 |
| Swift 终端壳（SwiftUI 原型） | ✅ 可交互，待换 SwiftTerm |

## 3. 总体架构

```
┌───────────────────────────────────────────────────────┐
│ Swift 层                                                │
│  SwiftTerm(VT100 渲染/键盘) · 多窗口 Scene · iOS 集成命令  │
│  (pickFolder/bookmark/open/pbcopy… 经 FFI 注册为 builtin) │
├───────────────────────────────────────────────────────┤
│ FFI 会话层 (core/src/lib.rs)                            │
│  每窗口: 专属线程 + tokio + brush Shell 实例              │
│  fd0/1/2 ↔ 真管道 ↔ SwiftTerm；带内 sentinel 协议         │
├───────────────────────────────────────────────────────┤
│ Shell 引擎: brush-core (vendor/, 跟随上游 + iOS 补丁)     │
│  bash 语法 · 展开 · 作业控制 · builtin 分派               │
├──────────────┬──────────────┬─────────────────────────┤
│ 命令层 T1     │ 命令层 T2     │ 命令层 T3                │
│ Rust 进程内   │ 语言运行时     │ wasm 分发                │
│ 手写+uutils+  │ CPython 3.14  │ wasmi 解释器             │
│ Rust CLI 生态 │ nodejs-mobile │ pkg install 任意 wasi 程序│
└──────────────┴──────────────┴─────────────────────────┘
```

## 4. 关键选型决策

### 4.1 Shell 引擎：brush-core（upstream-first）

- vendor 目录跟随上游 main + 我们的 iOS 分支；PR #1246 合并后转为纯 crates.io/git 依赖。
- 后续需要的上游改造按优先级：① `ShellExtensions` 增加 command-resolver hook（外部命令统一路由到我们的注册表，替代逐名枚举）；② iOS CI target。都以 PR 形式推上游，不养私有 fork。

### 4.2 命令层 T1：Rust 进程内命令

三个来源，统一注册为 brush builtin（`builtins::Command` trait）：

1. **手写**（已有 9 个）：小命令直接写，30-60 行/个，天然用 ExecutionContext 的管道 fd，无全局状态问题。适用：基础文件/文本命令。
2. **Rust CLI 生态直接嵌**（品质远超手写，作为库引入）：
   - `jaq`（jq 的纯 Rust 实现，可当库用）→ jq
   - `mlua`（Lua 5.4 绑定，vendored 编译）→ lua
   - `gitoxide` → git（对标 a-Shell 的 lg2）
   - `russh` → ssh/scp/sftp/ssh-keygen
   - `reqwest`/`hyper` → curl/wget 子集
   - `similar` → diff；`regex-lite` → grep 系（已用）
   - `image` crate → convert/identify 基础子集（对标 ImageMagick 常用路径）
3. **uutils coreutils**（覆盖长尾）：已验证约 60-70% 命令可接入，但其全局 stdio/cwd 需"大锁 + dup2"串行化（MVP 阶段可接受）或逐个 Context 化（后期）。8 个本质需 fork 的命令（env/nohup/nice/timeout/chroot/runcon/stdbuf/install）永久排除。
   - **决策：先手写 + 生态 crate 覆盖高频 40 个命令（无全局状态、可并发），uutils 只用于长尾补齐，不在关键路径上。**

### 4.3 Python：官方 CPython iOS XCFramework ⭐

调研结论（详见 §8 来源）：
- PEP 730 落地后 iOS 是 CPython 官方支持平台（Tier 3），**python.org 直接发布 iOS XCFramework（当前 3.14.6）**，含官方嵌入文档和 testbed。BeeWare Python-Apple-support 同源可作备选（多版本 + 附带 OpenSSL/libFFI 等依赖）。
- 集成方式：libPython 嵌入 + `python3` 注册为 builtin，进程内调 `Py_RunMain`/自定义 REPL；stdout/stderr 接会话管道。iOS 版 stdlib 官方就没有 subprocess/multiprocessing——与我们无 fork 架构天然对齐。
- **pip 策略**：进程内跑 pip；纯 Python wheel 从 PyPI 直装（PyPI 已接受 iOS wheel tag）；`--only-binary :all:` + 禁 build isolation 规避 spawn。**二进制 wheel（numpy 等）运行时安装因签名限制不可行**——常用二进制包 framework 化预打进 bundle（官方 `AppleFrameworkLoader` 机制），这是与桌面体验的已知差距（a-Shell 同样取舍）。
- **多窗口会话**：Python 3.14 子解释器（PEP 684 per-interpreter GIL + PEP 734）为主——一份 libPython，每窗口一个隔离解释器。坑：numpy/pandas 不支持子解释器 → 含重型 C 扩展的会话回退到"a-Shell 式多 dylib 副本"兜底（预置 2-3 份）。

### 4.4 Node + npm：nodejs-mobile 常驻实例 ⭐

调研结论：
- **nodejs-mobile 是唯一"真 Node"路线**（V8 jitless 构建，NodeMobile.xcframework），有 **Code App（thebaselab/codeapp，MIT，App Store 在售）** 的量产先例可直接抄作业。已知代价：版本停在 Node 18（EOL，社区维护慢）、解释执行慢约 40%。
- **硬约束：进程内单实例、终止后不能重启** → 架构上设计为 App 启动即拉起常驻 Node runtime，`node foo.js` / `npm install` / `npx` 作为任务派发进去（新 context/worker_threads），stdout 桥回对应会话管道。
- **npm 跑本体**：patch `child_process`（对 node 自身的 spawn 重定向为进程内任务）、默认 `--ignore-scripts`、屏蔽 native addon——照 Code App 的 shim 实现。
- LLRT/QuickJS 仅作为可选的轻量内部 JS 执行器（快速启动小脚本），不当 node 卖点；Bun/Deno/Hermes/txiki.js 路线全部排除（JIT 依赖或兼容层太薄）。

### 4.5 命令层 T3：wasm 运行时 + pkg 生态

- 运行时选 **wasmi**（纯 Rust 解释器，进程内集成零摩擦；wasmtime-Pulley 作为性能升级备选）。
- 提供 `wasm` 命令 + `pkg install`，兼容 wasi 命令分发（可直接受益于 a-Shell-commands 已有的 zip/xz/ffmpeg 等预编译包）。
- clang/TeX/perl 这类重型 C 生态**不做原生移植**，全部指到 wasm 通道（如 clang → 提示 `pkg install`，同 a-Shell 的 needLLVM 模式）。
- CPython 的 wasm32-wasi 版仅作沙箱执行备选，不承担主力（无线程/无扩展/无 socket）。

### 4.6 终端 UI：SwiftTerm

- 换掉原型的文本框：SwiftTerm 提供完整 VT100/xterm 语义（颜色、光标控制、全屏程序基础）。
- FFI 协议升级：输出流带内 sentinel（命令结束标记 + 退出码 + cwd），根治当前 150ms 延迟的竞态规避；stdin 逐键直通（raw mode 时）+ 行缓冲（canonical mode 时），termios 状态由 Rust 侧会话维护。
- 编辑器：先 helix（Rust，可库化探索）或 wasm vim；不承诺原生 vim。

### 4.7 iOS 集成命令（Swift 侧）

对齐 a-Shell 的差异化体验：`pickFolder`/`bookmark`/`jump`（安全书签跨沙盒访问）、`open`/`view`/`play`、`pbcopy`/`pbpaste`、`say`、`newWindow`、`config`。实现为 Swift 函数经 FFI 注册成 builtin（FFI 增加 Swift 回调注册接口）。

## 5. a-Shell 命令对齐矩阵（263 个 → 分层归属）

| 归属 | 数量(约) | 代表 | 说明 |
|---|---|---|---|
| brush 原生 builtin | 25 | cd echo export history pwd type alias env sh | 已免费获得 |
| T1 手写/已有 | 15 | ls cat grep head wc mkdir rm touch uname | 已完成 9 个 |
| T1 生态 crate | 45 | jq lua git ssh scp curl diff tar gzip xz find sed awk sort uniq tree xxd base64 sha* | 逐波次接入 |
| T1 uutils 长尾 | 50 | date du stat cksum split tee xargs mktemp realpath… | 大锁串行化 |
| T2 Python | 4 | python python3 pip + deactivate | M3 |
| T2 Node（新增，a-Shell 没有） | 3 | **node npm npx** | M4，差异化卖点 |
| T3 wasm 通道 | 40 | clang/lld/llc… ffmpeg ffprobe unrar zip 全部 TeX | pkg install |
| Swift 集成命令 | 25 | pickFolder bookmark open play say pbcopy config newWindow | M5 |
| 明确不做/延后 | 55 | perl 全家桶 ImageMagick 全量 taskwarrior TeX 原生 | wasm 通道兜底或放弃 |

## 6. 里程碑

- **M0 ✅ 原型验证**：brush-core iOS 化 + 102 用例全绿 + 上游 PR。
- **M1 终端化（骨架定型）**：SwiftTerm 接入、sentinel 协议、raw/canonical stdin、多窗口 Scene（每窗口一 Shell 实例）、xcodegen → 正式工程。验收：vim 级别的全屏 wasm 程序可运行前置条件（termios 语义）就绪，交互式 `read`/Ctrl-C 可用。
- **M2 命令宽度第一波**：T1 生态 crate 40 个高频命令 + 测试炮台扩到 300 用例（每命令≥3 用例）。验收：日常文件/文本/网络操作不出终端。
- **M3 Python**：CPython 3.14 XCFramework 嵌入、python3 REPL/脚本、pip 纯 Python 包、子解释器多会话。验收：`pip install requests && python3 -c "import requests"`。
- **M4 Node + npm**：nodejs-mobile 常驻实例、node/npm/npx 命令、child_process shim。验收：`npm install lodash && node -e "console.log(require('lodash').chunk([1,2,3,4],2))"`。
- **M5 生态与集成**：wasmi + pkg install、Swift 集成命令、uutils 长尾、App Store 准备（2.5.2 审核话术：开发工具、用户代码可见可编辑、无远程代码解锁功能）。

## 7. 风险清单

| 风险 | 等级 | 缓解 |
|---|---|---|
| nodejs-mobile 停在 Node 18 (EOL) | 高 | 接受现状（Code App 同款）；关注社区 v20 进展；LLRT 作长期备胎 |
| 二进制 wheel 运行时不可装（签名） | 中 | 预打常用包进 bundle；文档明示差距 |
| numpy 等不兼容子解释器 | 中 | 多 dylib 副本兜底；限制在主会话 |
| App 审核 2.5.2 波动 | 中 | 对齐 a-Shell/Pythonista 先例；审核说明模板 |
| brush PR 不被接受 | 低 | 维持薄 fork（补丁仅 86 行，rebase 成本低） |
| uutils 全局状态改造量 | 低 | 已降级为长尾补齐，不在关键路径 |
| V8 jitless / wasm 解释性能 | 低 | CLI 负载可接受；wasmtime-Pulley 备选 |

## 8. 调研来源（节选）

- Python：PEP 730/684/734/816；docs.python.org/3.14/using/ios.html；python.org/downloads/ios（官方 XCFramework 3.14.6）；beeware/Python-Apple-support；PyPI warehouse#17559（iOS wheel）；beeware.org/mobile-wheels
- Node：nodejs-mobile（v18.20.4，NodeMobile.xcframework，FAQ 单实例限制）；v8.dev/blog/jitless；thebaselab/codeapp（先例）；awslabs/llrt；orogene（已停摆，仅参考思路）
- Shell：reubeno/brush#1246（我们的 iOS 补丁 PR）；本仓库 core/src/selftest.rs（102 用例）
- a-Shell 架构分析与命令清单：见对话记录及 Resources/bin 提取（263 全量 / 167 mini）
