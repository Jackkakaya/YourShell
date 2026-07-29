# Upstream CLI integration status

目标：优先让上游命令自己解析 `argv` 并实现语义；YourShell 只桥接 session 的 cwd、env、stdio、取消和 iOS 生命周期。不能安全嵌入上游 `main` 时，才保留薄兼容层或库后端。

| 命令 | 当前接入 | argv/语义归属 | iOS 状态 | 明确边界 |
|---|---|---|---|---|
| curl | 官方 curl 8.1.2 tool + libcurl | 官方 curl CLI | 模拟器 HTTPS、重定向、输出、重复调用通过 | 未编入 SSH 类协议；SSH 由专用 Host 负责 |
| wget | 薄参数翻译 → 官方 curl CLI | TLS/HTTP/代理/重试/传输归 curl；仅 Wget 拼写由本地映射 | 下载、stdout、目录前缀、HTTPS 通过 | 不支持递归/镜像，明确报错 |
| tree | 本地纯 Rust walker | 7 个稳定参数由本地实现 | 重复调用安全，基础输出通过 | 不是完整 upstream tree；颜色、元数据、复杂排序未承诺 |
| zip | Rust `zip` crate | 本地薄 CLI + crate | 递归、更新、删除、排除、stdin 名单、store、回读通过 | 不支持加密、分卷；按要求不引入 WASM |
| git | vendored libgit2 (`git2`) | 本地 porcelain dispatcher + libgit2 | init/status/add/commit/log/diff/branch/checkout/fetch/pull/push/config/remote 具备 | 不是完整 Git；a-Shell 同样使用 `lg2` 替代完整 Git |
| python | 嵌入式 CPython 3.14 Host | CPython `Py_Main` 风格运行时 | pip、requests、NumPy、Pandas、Pillow、PDF、真实 python-pptx、重复调用通过 | 无 fork/subprocess；原生扩展需要 iOS wheel |
| node | NodeMobile 常驻 Host | Node runtime；命令经常驻 IPC | Node、npm init/install/require、重复调用通过 | 无桌面 child_process/native addon 保证 |
| ssh | `russh` 专用 Host | Rust SSH 协议栈 | 参数、认证、PTY、取消、多 Session 代码和单测具备 | 不宣称完整 OpenSSH 配置/转发/agent 兼容 |
| mosh | SSH bootstrap + Rust SSP/UDP Host | 专用协议实现 | wire、加密、分片、恢复状态单测通过 | 最终完成度仍需真实 mosh-server 互操作 |
| ocr | Apple Vision Host | 系统 Vision | 模拟器图片识别通过 | 仅 Apple 平台 |

## 统一适配形态

```text
Brush 解析 shell 语法
        |
        +-- 状态型 builtin（cd/export/read）直接修改当前 Session
        |
        +-- 上游 CLI argv
        |      |
        |      +-- CommandHost: cwd/env/fd/exit containment
        |      +-- upstream main(argv)
        |
        +-- Runtime/Protocol Host（Python/Node/SSH/Mosh/Vision）
```

`CommandHost` 当前为保护 C CLI 的进程级 cwd/env/fd 使用全局锁。Session 自身仍可并发；只有经过该桥的命令串行。后续 TODO 是把更多上游入口改造成显式 context/线程局部 stdio，逐步缩小锁的覆盖范围。

## 已验证基线（2026-07-29）

- 原生 Rust：331/331；全部 target 和并发测试通过。
- iOS 模拟器：完整电池首次 375/376；唯一失败为构建漏开 `vision` feature。
- 修正 Xcode 自动构建为 `python,node,vision` 后，OCR 独立真运行识别出 `HELLO YOURSHELL`。
- Python 模拟器通过：pip、requests 抓取热搜页、NumPy/Pandas 数据分析、Pillow、PDF、Flet iOS wheel 源安装 `python-pptx` 并生成/回读 PPTX。
- Node 模拟器通过：常驻重复调用、npm init、npm install、模块 require。
- 真机：此前版本已签名安装；最新代码仍需完成最终签名安装与命令矩阵。
