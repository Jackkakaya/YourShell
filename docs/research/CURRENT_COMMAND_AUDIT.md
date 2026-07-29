# 当前命令实现审计

本文件只审计当前源码，不读取旧 `docs`。目标不是引入新的执行体系，而是判断
现有命令是否符合以下原则：

1. Shell 状态命令由 Brush 实现。
2. 成熟外部 CLI 的 argv、flags、help、错误码尽量由上游实现。
3. YourShell 只维护 Session cwd/env/I/O 与上游入口之间的薄适配。
4. 没有标准 CLI，或必须与 App/runtime/协议深度结合时，允许专用 Host。
5. 不为了形式统一替换已经很薄、很正确的本地命令。

## 总体判断

| 类别 | 数量 | 判断 |
|---|---:|---|
| Brush Bash builtins | 61 | 符合；必须保持 Shell-native |
| uutils/coreutils | 74 | 符合；是标准的上游 multicall 转发 |
| 其他已转发上游 CLI | 10 | 符合：grep、sed、find、xargs、diff、cmp、awk、jq、npm、npx |
| 小型 Shell/App 命令 | 3 | 符合：which、clear、ocr |
| 专用 Host/runtime | 13 | 大体符合：SSH/Mosh、编辑器、Python、Node、Git 等 |
| 重新实现成熟 CLI | 10 | 不符合或部分符合：rg、tree、gzip、gunzip、curl、wget、tar、zip、unzip、sqlite3 |

这里的“10 个不符合”是当前最应该调研和替换的范围，不是 171 个命令全部
重做。

## 符合预期的实现

| 命令组 | 当前入口 | 为什么符合 |
|---|---|---|
| 61 个 Brush builtin | `BuiltinSet::BashMode` | `cd/export/read/source/...` 必须读写当前 Shell 状态，不能换成普通 CLI main |
| 74 个 uutils | `bundled_commands()[name](argv)` | flags、help、行为和退出码由 uutils 提供；adapter 没有重写命令 |
| grep | `uu_grep::uumain(argv)` | 上游完整 CLI parser |
| sed | `sed::uumain(argv)` | 上游完整 CLI parser |
| find/xargs | `findutils` 上游入口 | 上游完整 parser；只为 iOS 的 exec 行为增加 hook |
| diff/cmp | `diffutils` 上游入口 | 上游 parser；只修正 `exit`/返回码等嵌入问题 |
| awk | One True Awk `main(argc, argv)` shim | 直接进入成熟 CLI |
| jq | jq C CLI `main(argc, argv)` shim | 直接进入成熟 CLI |
| npm/npx | 常驻 Node runtime | argv 由 npm/npx 自己解析，不是 YourShell 重写 flags |
| python/pip | CPython/pip Host | runtime 生命周期必须由宿主管理；主要 CLI 语义仍交给上游 |
| ssh/scp/sftp/mosh | Rust 协议 Host | 直接使用 Session fd，避免进程全局状态；不是简单文件型 CLI |
| which | Brush-aware 本地命令 | 需要识别 builtin/function/PATH；普通外部 `which` 反而看不到 Shell 状态 |
| clear | 约 40 行 ANSI 命令 | flag 面极小，引入 ncurses/terminfo 上游得不偿失 |
| ocr | Apple Vision Host | 没有对应的标准 Unix CLI 上游 |

## 部分符合：应该保留，但要明确兼容边界

| 命令 | 当前实现 | 判断 | 改进方向 |
|---|---|---|---|
| git | libgit2 上的约 479 行 CLI 子集 | 不是完整 Git CLI，但官方 Git 强依赖子进程、helper、hook、pager | 明确标记为 Git subset；不要继续无边界追 flags；优先复用 libgit2 示例/解析层 |
| edit/vi/nano | 同一个本地编辑器引擎 | 是产品内编辑器，不是真正 Vim/Nano 兼容实现 | `edit` 保留；`vi`/`nano` 应标为兼容别名或重新命名，避免暗示完整兼容 |
| node | 常驻 NodeMobile dispatcher | 符合 NodeMobile 单 runtime 约束 | 保留，不应改成每次 `node_main` |
| python | CPython Host，再经过 process-state bridge | 上游 runtime 方向正确，但仍受全局 cwd/env/fd 锁影响 | 后续单独研究 CPython config/I/O 注入，不与普通 CLI 一起机械迁移 |

## 不符合预期：当前在重写成熟 CLI

| 命令 | 当前实现 | 主要问题 | 不用 WASM 的替代方案 | 判断 |
|---|---|---|---|---|
| rg | 约 616 行本地 Clap + ripgrep 底层 crates | 重写了大量官方 flags，高层语义容易漂移 | vendor ripgrep CLI 层，抽出 `run(argv)`；先接受 `command_host` 锁，再逐步注入 I/O | 可 Native，难度中 |
| tree | 本地 Clap + walkdir | 重新定义 tree 的显示、过滤和错误行为 | 接入 BSD/C tree 的 `tree_main`，或选择已有完整 Rust CLI 并暴露 main | 可 Native，难度低 |
| gzip/gunzip | 本地 Clap + flate2 | gzip 不只是 DEFLATE；文件名、时间戳、suffix、list、退出码都在重写 | 直接采用 ios_system 已验证的 NetBSD gzip port，`gzip_main(argc, argv)` | 可 Native，优先 |
| curl | 本地 Clap + ureq | 只覆盖 curl 很小子集；部分 flag 只是接受后警告 | 采用 a-Shell 已移植的 curl tool 层，调用 `curl_main(argc, argv)` | 可 Native，优先做构建/体积验证 |
| wget | 本地 Clap + ureq | 不是 GNU Wget 的下载、递归、认证、配置语义 | 若不接受 GPL/移植成本，明确为有限 downloader；也可取消 `wget` 名称；不能假装 curl 就是 wget | Native 可做，但产品决策优先 |
| tar | 本地 Clap + Rust `tar` crate | archive library 被重新包装成不完整 tar CLI；已有行为与 GNU/BSD 都可能不同 | 采用 ios_system 的 BSD tar port，`tar_main(argc, argv)` | 可 Native，优先 |
| zip/unzip | 本地 Clap + `zip` crate | 重写 Info-ZIP CLI；更新、编码、密码、属性、Zip64 行为难兼容 | Native 移植 Info-ZIP `zipmain/unzip`，拦截 exit/stdio 并重置 globals | 可 Native，难度中高 |
| sqlite3 | 本地极简 Clap + rusqlite | 缺少官方 dot commands、输出模式、参数、交互行为 | 编译 SQLite 官方 `shell.c`，暴露 `sqlite3_shell`/重命名 main | 可 Native，优先 |

## 不引入 WASM 是否做得了

可以。以上 10 个命令没有一个被证明“必须依赖 WASM”。

```text
Brush ExecutionContext
        ↓
现有 command_host::dispatch
        ↓
上游 native command_main(argc, argv)
```

a-Shell 的主线本来也是大量 Native framework：

- gzip/gunzip → BSD `gzip_main`
- curl → `curl_main`
- tar → BSD `tar_main`
- ssh/scp/sftp → 对应 native main
- Python → `python_main`

它把部分可选包做成 WASM，是一种打包和移植选择，不意味着 Native 无法实现。

在 YourShell 当前阶段，Native 路线还有一个现实优势：`command_host` 已经能为
面向进程的上游 CLI 提供 cwd/env/fd 桥。它有全局锁的并发限制，但能先保证
兼容性，并且不需要先增加新 runtime。

## Native 路线需要解决的共同问题

这些不是新架构，而是现有 `command_host` 接入上游 main 时本来就必须做的
移植审计：

| 问题 | 处理方式 |
|---|---|
| `main()` 符号 | 重命名为 `command_main()` 或暴露 Rust `uumain()` |
| `exit()` | 改为返回/longjmp 边界，绝不能结束 App |
| `getopt` 全局状态 | 每次调用前重置 `optind/opterr` 等 |
| static/global 状态 | 每次初始化/释放，必要时做 thread-local |
| stdin/out/err | 第一阶段继续由 `command_host` 的 `dup2` 提供 |
| cwd/env | 第一阶段继续由 `command_host` 加全局锁映射 |
| fork/exec/system | 禁用、报明确错误，或映射回 Brush registry |
| signal handler | 调用后恢复；现有 SIGPIPE 防护继续保留 |
| 重复调用 | 至少连续运行两次、错误后再成功一次 |
| 并发 | ProcessGlobal 命令暂时串行；不要把“Native”误写成“线程安全” |

## 建议的最小改进顺序

不先增加 `WasiBackend`，也不先重构整个 registry。

| 顺序 | 工作 | 原因 |
|---:|---|---|
| 1 | 固化“禁止为成熟 CLI 新增本地 Clap flags”的规则 | 立即停止债务增长 |
| 2 | 删除 `commands_ext.rs` 中已经不再注册的旧 `JqCommand` | 当前 jq 已走官方 C CLI，旧实现是重复/死代码 |
| 3 | Native 接入 SQLite `shell.c` | 官方入口清晰，能一次删除极简假 CLI |
| 4 | Native 接入 BSD gzip/gunzip | a-Shell/ios_system 已有同平台参考 |
| 5 | Native 接入 BSD tar | 同样有 iOS 已验证参考 |
| 6 | Native 接入 curl tool | 复用官方 parser，测量二进制体积和 TLS |
| 7 | 分别评估 rg、tree、Info-ZIP | 都可 Native，但 patch 成本不同 |
| 8 | 最后处理 wget 命名/兼容目标 | 先决定要 GNU Wget，还是有限 downloader |
| 9 | 命令替换稳定后再处理全局锁 | 兼容性和并发隔离分开解决 |

## 修正后的结论

当前最重要的问题不是缺少 WASM，而是有 10 个命令仍在“底层库之上重写成熟
CLI”。最直接的改进是继续沿用已经成功的 uutils/awk/jq 模式：

```text
命令名 → 薄 adapter → 上游 native CLI entry
```

第一阶段完全可以接受这些 Native CLI 继续经过现有全局锁。全局锁是并发优化
问题，不应该阻止先删掉手写 flags，也不应该被用来论证必须引入 WASM。

只有某个具体上游经过 Native 构建、exit/global/stdio 审计后，确实证明维护
成本不可接受，才需要重新比较 WASM、保留有限实现或不提供该兼容名。

## 已确认的项目原则

1. 先审计现有实现是否复用上游 CLI，再考虑增加任何执行后端。
2. 默认路线是 Native 上游入口加现有 `command_host`。
3. 全局锁与 CLI 兼容改造分开处理；第一阶段允许 Native 命令继续串行。
4. 不因为 a-Shell 某个可选包使用 WASM，就推导 YourShell 也必须引入 WASM。
5. 只有具体命令的 Native 路线经过实际验证后不可接受，才重新比较其他方案。
6. 增加命令前先确认用户价值、上游入口、许可证、iOS 可行性和维护成本。
