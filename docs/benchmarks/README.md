# YourShell 性能基准

在设备/模拟器上运行：`node ~/Documents/node_bench.js` 和 `python3 ~/Documents/python_bench.py`
（脚本把结果写到 `~/Documents/nodeperf.txt` / `pyperf.txt`）。

## 参考数据（iOS 模拟器，跑在 Mac CPU 上，2026-07-24）

### Node 18 (V8 jitless) — 相对 Mac 原生 JIT 的倍数
| 操作 | 模拟器 | Mac(JIT) | 倍数 |
|---|---|---|---|
| sort 50万浮点 | 285ms | 150ms | 1.9x |
| JSON 20万键往返 | 230ms | 192ms | 1.2x |
| 正则 20万匹配 | 26ms | 7ms | 3.7x |
| sha256 ×2万 | 25ms | 9ms | 2.8x |
| fib(32) 递归 | 161ms | 13ms | 12.4x |

规律：原生 C++ 操作(JSON/sort/crypto) 1-2x；纯 JS 计算 3-12x；混合负载 2-3x。

### Python 3.14 — 零 JIT 惩罚（原生 CPU 速度）
| 操作 | 模拟器 | Mac(Py3.9) |
|---|---|---|
| PIL 500×500 逐像素 | 40ms | 38ms（几乎相同→证明原生速度）|
| PPTX 20 页 | 356ms | 2882ms |
| PIL 柱状图 | 18ms | 159ms |
| PDF 30 页+图 | 310ms | 1140ms |

CPython 本就是纯字节码解释器，无 JIT，所以 iOS 禁 JIT 对它无影响。

### npm install（网络延迟主导，非磁盘/CPU）
npm --timing 拆解：86% 是依赖树解析(registry 往返)，unpack 仅 43ms。
换镜像 registry.npmjs.org 7176ms → npmmirror.com 1243ms（5.8x）。已默认 npmmirror。
