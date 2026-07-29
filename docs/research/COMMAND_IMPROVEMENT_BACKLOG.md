# 命令改进总清单

> 本文件保留需求池用途。经过进一步上游调研后的实际判断、准入条件和执行
> 顺序，以 [`RESEARCHED_COMMAND_PLAN.md`](./RESEARCHED_COMMAND_PLAN.md)
> 为准。

本清单合并两类工作：

1. YourShell 已有命令，但当前在底层库上重新实现了成熟 CLI。
2. a-Shell 已有、YourShell 缺失，而且对 iOS Shell 确实有价值的命令。

统一原则：

```text
优先上游 Native CLI main/uumain
        ↓
薄 adapter 接现有 command_host
        ↓
不在 YourShell 重写 flags
```

WASM 不作为前置方案。第一阶段允许上游 Native CLI 继续经过现有
`process_state_lock`；兼容性替换与跨 Session 并发优化分开处理。

## A. 已有但实现不够好的命令

| 优先级 | 命令 | 当前问题 | 准备怎么做 | 目标 |
|---:|---|---|---|---|
| A0 | `sqlite3` | `rusqlite` 上自建极简 CLI；缺官方 dot commands、输出模式和大量参数 | 编译 SQLite 官方 `shell.c`，将 main 改为可重复调用入口；用薄 adapter 转发 argv | flags、dot commands、help、退出码交给 SQLite |
| A0 | `gzip`, `gunzip` | `flate2` 上自建 Clap；文件名、时间戳、suffix、list 等语义不完整 | 复用 ios_system/NetBSD gzip Native port；两个名字共享 `gzip_main`，由 argv[0] 分派 | 删除两套本地 flag schema |
| A0 | `tar` | Rust `tar` crate 上自建 CLI；只覆盖 GNU/BSD tar 子集 | 复用 ios_system 的 BSD `tar_main`；审计 exit、stdio、外部压缩器 | 上游负责 tar parser 和 archive 行为 |
| A0 | `curl` | 当前仍是 ureq-backed subset，但核心请求 flags 已覆盖 | 已补齐 `--connect-timeout`/`--max-time`，继续以 iOS TLS/CA 评估为边界；暂不伪装成完整 curl | 17/17 flag coverage；后续再决定官方 curl tool 移植 |
| A1 | `rg` | 约 616 行本地 Clap，重新声明官方 ripgrep flags | vendor 官方 ripgrep CLI 高层，抽取 `run(argv)`；第一阶段可走现有 command host | 保留完整官方 parser，不再维护本地 flags |
| A1 | `tree` | `walkdir` 上自建 tree CLI 和输出语义 | 选择 BSD tree 或成熟 Rust tree CLI，暴露 Native main/run | 删除本地 TreeCommand |
| A1 | `zip`, `unzip` | `zip` 创建和解压的上游前端并非同一实现 | `unzip` 使用 libarchive `bsdunzip`；`zip` 保留 iOS-safe Rust crate，并按行为测试增量增强 | 明确创建/解压边界，不引入不兼容的 Info-ZIP 移植 |
| A2 | `wget` | `ureq` 上自建有限 downloader，却使用完整 Wget 名称 | 先决定兼容目标：接受 GPL 则研究 GNU Wget Native；否则明确改名/标注有限 downloader，并冻结 flags | 不再以有限实现暗示完整 Wget |

### A 类实施注意

每个上游 Native CLI 都必须检查：

- `exit()` 不得结束 App；
- `getopt` 和 static/global 状态每次运行前重置；
- cwd/env/fd 第一阶段由现有 `command_host` 映射；
- `fork/exec/system/popen` 明确禁用或接回 Brush；
- 连续成功两次、失败后成功、跨 Session 调用都要测试；
- help、stdout、stderr、退出码与选定上游版本比较。

## B. 已有但定位需要修正的命令

这些不一定需要换实现，主要是避免错误的兼容承诺。

| 优先级 | 命令 | 当前问题 | 准备怎么做 |
|---:|---|---|---|
| B0 | `jq` 旧 Rust 实现 | `commands_ext.rs` 仍残留不再注册的 `JqCommand`，当前实际已走官方 jq C CLI | 确认无引用后删除死代码和无用依赖，保留 `jq_adapter` |
| B0 | `vi`, `nano` | 两个名字实际进入同一个本地编辑器，不是完整 Vim/Nano | 保留 `edit`；为 `vi/nano` 明确显示兼容说明，或后续取消误导性别名 |
| B1 | `git` | libgit2 CLI 子集容易被理解为完整 Git | 冻结无边界 flag 扩展；列出支持的子命令，help 明确为 subset；继续复用 libgit2 |
| B1 | `python` | runtime 方向正确，但经过 process-global cwd/env/fd bridge | 保留 CPython Host；以后单独研究 CPython config/I/O 注入，不与普通 CLI 一起改 |
| B1 | `ssh/scp/sftp/mosh` | 专用 Host 合理，但各自注册和能力描述分散 | 不替换协议实现；后续只统一外层注册、取消和 terminal 能力描述 |
| B1 | `node/npm/npx` | 常驻 Node 方向正确 | 保持现状；补充上游版本、单 runtime 限制和兼容测试 |

## C. 当前缺失，建议第一批新增

| 优先级 | 命令 | 准备怎么做 | 为什么值得做 |
|---:|---|---|---|
| C0 | `stat` | 已直接接入官方 `uu_stat` 0.8.0，并复用现有 process-shaped uutils adapter | 脚本和文件诊断的基础命令 |
| C0 | `egrep` | 复用现有 `uu_grep`，按命令名注入 `-E` | 几乎零成本的历史兼容名 |
| C0 | `fgrep` | 复用现有 `uu_grep`，按命令名注入 `-F` | 同上 |
| C0 | `pbcopy` | Swift Clipboard Host，从 stdin 写剪贴板 | iOS Shell 高频集成 |
| C0 | `pbpaste` | Swift Clipboard Host，将剪贴板写 stdout | 同上 |
| C0 | `open` | Swift `UIApplication.open` Host，处理 URL/文件 | iOS 终端核心能力 |
| C0 | `openurl` | `open` 的 URL 兼容入口，不另写 parser | 与 a-Shell 命令兼容 |
| C1 | `bc`, `dc` | 复用 ios_system `bc_ios` Native port；一次注册两个名字 | 标准计算器，a-Shell 已验证 iOS 可行 |
| C1 | `dig`, `host`, `nslookup` | 研究 ios_system `network_ios` 工具族，统一 Native adapter | DNS 诊断基础能力 |
| C1 | `nc` | 接入 BSD/ios_system `netcat_main` | TCP/UDP 调试常用 |
| C1 | `ssh-keygen` | 优先只移植 OpenSSH keygen 模块；若依赖过大，选 OpenSSH 格式兼容的成熟 Rust CLI | 与现有 SSH 能力配套 |

## D. 当前缺失，建议第二批新增

| 优先级 | 命令 | 准备怎么做 | 前置验证 |
|---:|---|---|---|
| D0 | `cal`, `ncal` | BSD `cal_main`，argv[0] 分派两种模式 | 日期/locale 输出 |
| D0 | `hexdump` | 先找 uutils/uu_hexdump；否则 BSD Native CLI | format-string parser 必须复用上游 |
| D0 | `strings` | 优先小型 BSD strings Native CLI | 二进制格式依赖和 Unicode 行为 |
| D0 | `getopt` | util-linux/BSD getopt CLI；不要与 Brush `getopts` 混淆 | 许可证、GNU 兼容模式 |
| D0 | `column` | BSD 或许可证合适的 util-linux CLI | Unicode 宽度、表格模式 |
| D1 | `ping` | ios_system/BSD ping Native main | iOS ICMP socket 权限、timeout、cancel |
| D1 | `whois` | ios_system network Native main | 网络取消、默认服务器 |
| D1 | `say` | Swift `AVSpeechSynthesizer` Host | 异步完成、取消、voice 参数 |
| D1 | `uptime` | 小型系统 Host | iOS 可提供的信息是否足以匹配命令名 |
| D1 | `chflags` | BSD Native CLI | iOS 文件系统支持的 flags |
| D1 | `ed` | ios_system text framework Native main | 交互 stdin、全局状态重置 |
| D1 | `xz`, `lzmadec` | Native 接入 xz-utils 工具族 | 许可证、体积、重复调用 |

## E. 可选增强，不进入基础批次

| 命令/能力 | 准备怎么做 |
|---|---|
| `age`, `minisign` | 分别评估官方 CLI 能否抽取 Native argv 入口；作为安全工具可选 feature |
| `col`, `colrm`, `jot`, `look`, `lam`, `rs` | 按 BSD textutils 工具族整体评估，不逐个手写 |
| `imgcat` | 先定义终端图片协议和 App UI 支持，再接命令入口 |
| `banner`, `figlet`, `morse`, `ufetch` | 只在产品需要展示/趣味工具时加入 |

## F. 暂不计划加入

| 命令 | 原因 |
|---|---|
| `setenv`, `unsetenv` | Bash 已有 `export`、`unset` |
| `compress`, `uncompress` | `.Z` 老式格式，当前优先级低 |
| `ifconfig` | iOS 无法提供桌面 Unix 的完整接口配置语义 |
| `rlogin`, `telnet` | 明文老旧协议，已有 SSH |
| `wol` | 场景较窄，等待实际需求 |
| LLVM/Jupyter/Pandoc/TeX/Web 编辑器等 | 属于大型产品能力，不是基础命令补齐 |

## 建议执行顺序

```text
第 0 批：规则和清理
    禁止新增成熟 CLI 的本地 flags
    清理旧 JqCommand（已完成）
    明确 git / vi / nano 的兼容边界

第 1 批：替换最明确的不合格实现
    sqlite3
    gzip / gunzip
    tar
    curl

第 2 批：低成本补齐
    stat
    egrep / fgrep
    pbcopy / pbpaste
    open / openurl

第 3 批：继续替换本地 CLI
    rg
    tree
    zip / unzip
    wget 决策

第 4 批：新增 Native 工具族
    bc / dc
    dig / host / nslookup
    nc
    ssh-keygen

第 5 批：扩展基础工具
    cal / ncal
    hexdump / strings / getopt / column
    ping / whois / xz
```

## 每个命令进入实施前的记录模板

| 字段 | 内容 |
|---|---|
| 命令名/aliases | 实际注册名 |
| 现状 | 当前实现或缺失 |
| 兼容目标 | GNU、BSD、官方 upstream 或明确 subset |
| 上游项目和固定版本 | repo/tag/commit |
| 许可证 | App 分发是否接受 |
| 可调用入口 | `main`、`uumain`、`run` |
| 最小 patch | exit/getopt/global/I/O/fork/exec |
| 接入后端 | Brush、command_host、专用 Host |
| iOS 限制 | socket、PTY、系统 API、文件权限 |
| 验收 | flags/help/stdout/stderr/exit/repeat/pipeline/concurrency |
