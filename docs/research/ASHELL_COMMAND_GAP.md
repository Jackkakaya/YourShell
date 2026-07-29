# a-Shell 与 YourShell 命令差集

对比范围：

- a-Shell 内置基础命令：`ios_system/Resources/commandDictionary.plist`
- a-Shell 可选包目录：`a-Shell-commands/list`
- YourShell：当前 `COMMAND_MATRIX.md` 中的 171 个注册名

注意：可选包目录列的是**包名**，不是精确的最终命令名。比如 `zip` 包还会
安装 unzip 等工具。因此本文件把“内置命令”和“可选产品能力”分开讨论。

官方来源：

- [ios_system commandDictionary](https://github.com/holzschu/ios_system/blob/master/Resources/commandDictionary.plist)
- [a-Shell-commands package list](https://github.com/holzschu/a-Shell-commands/blob/master/list)
- [a-Shell README](https://github.com/holzschu/a-shell)

## a-Shell 内置、YourShell 当前没有的命令

| 类别 | 命令 |
|---|---|
| 计算器 | `bc`, `dc` |
| 文件/元数据 | `chflags`, `stat` |
| 旧式压缩 | `compress`, `uncompress` |
| 文本工具/别名 | `ed`, `egrep`, `fgrep`, `md5` |
| DNS/网络诊断 | `dig`, `host`, `nslookup`, `nc`, `ping`, `whois` |
| 网络/远程旧工具 | `ifconfig`, `rlogin`, `telnet`, `wol` |
| SSH 工具 | `ssh-keygen` |
| iOS/App 集成 | `open`, `openurl`, `pbcopy`, `pbpaste`, `say` |
| csh 风格环境命令 | `setenv`, `unsetenv` |
| 系统信息 | `uptime` |

共 29 个名字。其中一些只是现有能力的另一种命令拼写，并不都值得新增。

## 推荐新增：第一优先级

这些是常见、独立、价值明确的命令，并且有适合 Native 接入的上游。

| 命令 | 推荐上游/实现 | 接入方式 | 原因 |
|---|---|---|---|
| `stat` | BSD stat 或合适的 uutils stat crate | `stat_main/uumain(argv)` | 脚本和文件诊断常用；不应手写格式字符串 parser |
| `bc`, `dc` | ios_system 的 `bc_ios` port | `bcdc_main(argc, argv)`，按 argv[0] 分派 | a-Shell 已有 iOS Native 实证；两个命令一次接入 |
| `ssh-keygen` | a-Shell 的 OpenSSH port，或与现有 russh key 类型配套的成熟 CLI | 优先复用上游 parser；密钥读写接现有 Session cwd | 当前已有 SSH，缺 key 管理会形成明显能力断层 |
| `dig`, `host`, `nslookup` | ios_system `network_ios`，或同一 DNS 上游工具族 | 一个 native network adapter 注册三个入口 | DNS 诊断高价值；应按工具族一次接入 |
| `nc` | ios_system netcat/BSD netcat | `netcat_main(argc, argv)` | 通用 TCP/UDP 诊断；需确认 iOS socket 与取消行为 |
| `pbcopy`, `pbpaste` | Swift Clipboard Host | 两个极薄的 App builtin | iOS Shell 高频能力，没有必要引入外部库 |
| `open` | Swift `UIApplication.open` Host | App builtin | 打开 URL/文件是 iOS 终端的核心集成能力 |

### 具体建议

`stat` 应先检查当前 uutils 版本为什么没有被
`brush-coreutils-builtins` 收录。如果上游已有 `uu_stat::uumain`，优先扩展
现有 registry，而不是增加新的 adapter。

`bc`/`dc`、DNS 工具族、`nc` 可以直接研究 a-Shell 已经使用的 Native
framework 源码和 patch。这里借鉴的是它的 `command_main(argc, argv)`
移植成果，不是它的整个 shell dispatcher。

`ssh-keygen` 不应为了复用 parser 而同时引入第二套 SSH runtime。先调查能否
只移植 OpenSSH keygen 所需模块；如果依赖面过大，再选择一个成熟的纯 Rust
key CLI，并保持 OpenSSH key 格式兼容。

## 推荐新增：低成本兼容名

| 命令 | 实现方式 | 说明 |
|---|---|---|
| `egrep` | 注册到现有上游 grep，并注入 `-E` | 历史兼容名；不新增 parser |
| `fgrep` | 注册到现有上游 grep，并注入 `-F` | 历史兼容名；不新增 parser |
| `md5` | 优先 BSD md5 native main | BSD `md5` 输出/flags 与 GNU `md5sum` 不完全相同，不应简单 alias 后宣称完整兼容 |
| `openurl` | alias 到 `open` 的 URL 模式 | App 自有命令，语义可明确 |

`egrep`/`fgrep` 最好在 grep adapter 的命令名分派层实现，仍由 `uu_grep`
解析其余 argv。

## 建议新增：第二优先级

| 命令 | 推荐路线 | 需要先验证 |
|---|---|---|
| `ping` | ios_system/BSD ping native main | iOS entitlement、ICMP socket 权限、取消和超时 |
| `whois` | ios_system network tool native main | socket、默认服务器选择、编码 |
| `say` | Swift `AVSpeechSynthesizer` Host | 阻塞/异步语义、取消、语言和 voice 参数 |
| `uptime` | 小型 App/system builtin | iOS 能提供的 uptime/load 信息是否与 Unix 语义一致 |
| `chflags` | BSD chflags native main | iOS 文件系统实际支持哪些 flags |
| `ed` | ios_system text framework 的 native main | 交互 stdin、signal、重复调用 |

`say` 与 `uptime` 没有必要硬套 Native CLI：它们依赖 iOS 系统 API，专用
Host 才是最薄的正确实现。

## 暂不建议新增

| 命令 | 原因 |
|---|---|
| `setenv`, `unsetenv` | YourShell 是 Bash 语义，已有 `export`/`unset`；增加 csh 拼写价值低 |
| `compress`, `uncompress` | `.Z` 格式已属兼容性需求，现代脚本价值低；可在真实需求出现时整组接入 |
| `ifconfig` | iOS 无法提供桌面 Unix 上完整的接口配置能力；只读网络信息可另做明确命令 |
| `rlogin`, `telnet` | 明文、老旧协议；已有 SSH |
| `wol` | 场景较窄，可作为后续小型网络命令 |

这里“不建议”表示不进入默认内置集合，不代表技术上无法实现。

## a-Shell 可选包中值得评估的基础 CLI

去掉 YourShell 已有的 `base64`、`comm`、`csplit`、`cut`、`expand`、`false`、
`fmt`、`fold`、`git`、`join`、`mktemp`、`nl`、`paste`、`printf`、`rg`、
`seq`、`split`、`sqlite3`、`unexpand`、`which`、`zip` 后，仍有一批可作为
基础命令评估：

| 优先级 | 包/命令 | 推荐 Native 路线 |
|---|---|---|
| 高 | `cal`/`ncal` | BSD `cal_main`，两个名字共享实现 |
| 高 | `column` | BSD util-linux/BSD CLI main；注意许可证选择 |
| 高 | `hexdump` | BSD hexdump CLI main；也检查是否能扩展 uutils 集合 |
| 高 | `getopt` | util-linux getopt CLI；与 Shell builtin `getopts` 是不同命令 |
| 高 | `strings` | LLVM/BSD strings CLI；优先小型 BSD 实现 |
| 高 | `xz`/`lzmadec` | xz-utils CLI native port；一次接入工具族 |
| 中 | `col`, `colrm` | BSD textutils native main |
| 中 | `jot` | BSD jot native main；GNU 环境可用 `seq` 替代一部分 |
| 中 | `look`, `lam`, `rs` | BSD textutils native main |
| 中 | `age`, `minisign` | 官方 Rust/C CLI 抽取 native entry；属于现代安全工具 |
| 中 | `imgcat` | App terminal image protocol + Swift/UI 支持，不能只接 CLI parser |
| 低 | `banner`, `figlet`, `morse`, `ufetch` | 趣味/展示工具，不应优先于基础诊断能力 |

## 可选包中不应直接当作“缺失命令”的能力

以下项目体积、UI 或运行时属性明显，应单独做产品决策：

- 编辑器/Web UI：`ace-editor`, `codemirror`, `monaco-editor`, `kilo`, `nnn`
- 语言/编译器：`f2c`, `gawk`, `llvm`, `llvm-18`, `qjs`, `scheme-s7`
- 文档/科学环境：`graphviz`, `jupyter`, `pandoc`, `texlive-*`
- 数据转换：`json2csv`, `markdown-it`
- 其他大型工具：`swift-format`, `xmlcatalog`, `xmllint`, `xsltproc`
- 账户/认证工具：`gnu-pw-mgr`, `oathtool`

它们不是简单补一个 shell builtin 就结束，不能与 `stat`、`bc`、`dig` 放在
同一个实施批次。

## 推荐实施批次

```text
Batch 1：低风险、直接补齐
    egrep / fgrep / pbcopy / pbpaste / open / openurl

Batch 2：复用上游 Native CLI
    stat / bc / dc / cal / ncal / hexdump / strings

Batch 3：网络工具族
    dig / host / nslookup / nc / whois

Batch 4：与现有 Host 深度结合
    ssh-keygen / ping / say

Batch 5：按真实需求选择
    column / getopt / xz / age / minisign / ed
```

每一批仍遵守同一规则：

```text
先找上游 CLI main/uumain
    ↓
确认许可证与 iOS 构建
    ↓
薄 adapter 接现有 command_host 或专用 App Host
    ↓
重复调用、错误码、管道、Session 测试
```

不因为 a-Shell 提供某个命令就直接照搬，也不在 YourShell 重新定义成熟 CLI
的 flags。
