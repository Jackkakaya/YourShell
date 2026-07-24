# 已知 Bug

## [已澄清·非 bug] 裸 `npm` 返回 exit 1
- 报告：2026-07-24
- 结论：**这是 npm 的标准行为，不是 bug。** 无参数的 `npm` 会打印用法并以 exit 1 退出（提示需要子命令）。实测 Mac 原生 npm 同样 `rc=1`。
- 佐证（模拟器实测）：
  - `npm`（无参数）→ exit 1 + usage（与 Mac 一致）
  - `npm --version` → 10.8.2, rc=0
  - `npm ls` → rc=0
  - `npm install is-odd` → rc=0（换 npmmirror 镜像后 591ms）
- 同类：`git`、`pip` 等无参数也常以非 0 退出，属正常 CLI 约定。

## [观察] npm/node 输出偶发重复行
- 现象：某些 npm 命令输出出现重复行（如 "added 2 packages" 打印两次）。
- 疑因：main.js dispatcher 的完成检测/flush 逻辑可能重复发送尾部输出。
- 影响：仅显示层，退出码与安装结果正确。低优先级，待查。
