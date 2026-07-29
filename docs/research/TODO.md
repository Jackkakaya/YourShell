# TODO

## 解除进程型命令的全局串行锁

当前 `core/src/command_host.rs` 为兼容面向进程实现的命令，会在执行期间
临时替换进程级 cwd、环境变量和 fd 0/1/2。由于这些状态由整个 App 共享，
所有经过 `process_state_lock` 的命令会跨 Shell Session 串行执行。

目标：

```text
同一 Session 内命令串行
不同 Session 之间命令并发
```

需要逐步将命令从进程全局状态迁移到 Session 级上下文：

```text
CommandContext {
    argv,
    cwd,
    env,
    stdin,
    stdout,
    stderr,
    cancellation,
    terminal
}
```

执行命令时不再调用全局 `chdir`、`setenv` 或 `dup2`。优先迁移长时间持锁、
交互式或高频命令；在所有相关上游实现支持注入 cwd/env/I/O 前，保留全局锁
作为正确性保障。

当前边界：

- `ssh/scp/sftp/mosh` 等自身持有 Session I/O 的 Host 不经过
  `command_host::process_state_lock`，可以跨 Session 并发。
- `command_host::dispatch` 仍必须全程持锁：它用 `dup2` 映射进程 fd，并临时
  修改 cwd/env；缩短锁范围会把输出、cwd 或环境串到其他 Session。
- `tests/concurrency.rs` 目前验证的是隔离正确性（4 Session × 25 轮），不是
  并行度；在没有注入 API 前不应把它称为 Session 并发证明。
- `pbcopy/pbpaste/open/openurl` 已完成第一处迁移：它们直接使用 Brush 的
  `ExecutionContext` 和 Host callback，不再调用 `command_host::dispatch`，
  因此不触碰进程级 fd/cwd/env，也不占用全局锁。
- `CommandMain` 现在明确表示“进程形状入口”：任何继续使用它的 adapter
  都必须接受全局锁；Session-safe 命令应改写为直接消费 `ExecutionContext`，
  不得仅因为已有 `main(argc, argv)` 就复用该桥接层。

迁移顺序：

1. 为 `CommandMain` 增加显式 `IoContext` 入口，先迁移不依赖 libc fd 的纯 Rust 命令；
2. Native CLI 逐个增加 cwd/env/stdin/stdout/stderr 注入 seam；
3. 只有完全不触碰进程全局状态的命令才允许绕过全局锁；
4. 增加带 barrier 的并发测试，证明两个 Session 能同时运行。

已增加 `tests/concurrency_safe.rs`：4 个 Session 在 barrier 后同时运行
Session-safe 的 `git --version`，输出均保持隔离；进程型命令仍由原有
`tests/concurrency.rs` 单独覆盖。
