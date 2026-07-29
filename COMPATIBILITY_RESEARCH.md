# CLI compatibility research conclusions

## 决策

| 命令 | 调研结论 | 采用方案 | 原因 |
|---|---|---|---|
| curl | a-Shell 确实编译完整 curl tool；官方源码可静态构建 | 官方 curl 8.1.2 CLI | 不再手写 curl flags，收益最大 |
| wget | a-Shell 没有可直接复用的 GNU Wget 原生入口 | Wget 常用拼写薄映射到 curl | 复用成熟 HTTP/TLS 引擎，不引入 WASM 或第二套网络栈 |
| tree | upstream C 实现含大量进程全局变量及直接 `exit()` | 保留 Rust 实现 | 未找到可靠 invocation reset；直接嵌入会破坏多次调用 |
| zip | a-Shell 的 Info-ZIP 包是 wasm3；项目要求继续 Rust zip crate | 保留 Rust `zip` | iOS 安全、体积和重复调用可控，不为完整 flags 引入 WASM |
| git | a-Shell 的 `git` 实际是 shell wrapper，转发到 `lg2`，且明确并非 100% Git 兼容 | 保留 libgit2 dispatcher | 与成熟 iOS shell 的核心取舍一致 |
| python | iOS 必须由嵌入式 runtime + iOS wheel 生态完成 | CPython Host | 可执行纯 Python；原生扩展由预编译 wheel 解决 |
| node | Node 只能初始化一次 | NodeMobile 常驻进程内 runtime + IPC | 避免每条命令重新初始化和生命周期崩溃 |
| ssh | OpenSSH CLI 强依赖 Unix 进程、agent、TTY、配置生态 | `russh` Host | Session、取消和交互更适合显式异步连接 |
| mosh | 不只是一个 argv parser，而是 SSH bootstrap + UDP roaming + terminal state sync | 专用 Host | 必须围绕 iOS 网络切换和前后台生命周期设计 |

## 不做的错误统一

“全部转成 uutils 模式”适合满足以下条件的命令：上游提供可重入的 `main(args)`/`uumain(args)`、退出不会杀进程、stdio/cwd/env 可桥接、无后台生命周期。

以下情况不能机械套用：

- `cd/export/read` 必须修改当前 Brush Session，放到隔离进程语义就丢状态。
- Python/Node 是 runtime 生命周期，不是一次函数调用。
- SSH/Mosh 是长连接协议 Host，需要取消、窗口变化、网络迁移。
- tree upstream 的进程全局状态和 `exit()` 没有安全 reset。
- a-Shell zip 依赖 WASM，而本项目明确选择 Rust crate。

## 兼容声明原则

- 官方 parser 接入的命令可以按上游 flags 验收。
- 薄翻译层必须对不支持项明确失败，不能静默接受。
- 库后端命令只声明已测试的 porcelain/subset。
- 单元测试不能替代真实网络协议互操作；Mosh 仍需 pinned server live test。
