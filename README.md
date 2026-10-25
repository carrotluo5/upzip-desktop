# Unparanoia（帕罗诺亚解包器）

解密 **Paranoia File Encryption v4**（SSEFE）加密文件，并把解密出的内层压缩包继续解出来的
**GUI + CLI** 工具。前身为 `unparanoia`（只解密）与 `upzip`（批量解压/伪装压缩包）：
v0.2.0 合并为一个产品，v0.3.0 起统一更名为 **Unparanoia（帕罗诺亚解包器）**。

- Rust 核心 + Tauri v2 GUI（React 19 + TypeScript + Vite）
- 随附 7-Zip 25.01 官方二进制（子进程调用，不注入进程、不链接其代码）
- 支持批量、拖放、并发、暂停/继续/取消、级联解包、前缀 ZIP 兜底
- **多密码密码库**（明文保存在本机 config.json，0600）+ **逐层多密码重试**
- **分段压缩包**（`.7z.001` / `.zip.001` / `.part1.rar`，含 `.enc` 分卷自动聚合）
- **Windows 单 exe**：GUI/CLI 内嵌 7-Zip 25.01 与前端，不再附带 sidecar
- 明暗主题（跟随系统 / 浅色 / 深色）、`--licenses` 查看 7-Zip 许可证
- 日志落盘 `<configDir>/logs/unparanoia.log`（2 MiB 轮转）+ 设置页「诊断信息」
  （复制诊断 JSON / 打开日志目录 / 清除处理记录）
- 不联网；口令只在本机存储（明文 JSON，权限 0600），详见「安全模型」

## 功能

| 能力 | 说明 |
|---|---|
| Paranoia v4（SSEFE）解密 | AES-256-CTR + Argon2id KDF + BLAKE3 MAC + 32 字节 checkcode，逐字节校验 |
| 内层 ZIP 自动解压 | SSEFE 明文就是 ZIP；解密后按普通压缩包继续解（ZIP 校验和由 zip crate 校验） |
| 普通压缩包批量解压 | ZIP / 7z / RAR / tar / gz / xz / bz2 / cab / iso 等 7-Zip 支持的格式 |
| 伪装压缩包 | 魔数/EOCD/PE overlay 扫描 + 7-Zip 列表交叉确认 + 前缀数据 carve |
| 前缀 ZIP（7-Zip 25.01 已知拒绝） | 自动切片/偏移兜底，例如 MP4 头 + ZIP 载荷 |
| 嵌套级联 | 内层压缩包递归展开，默认深度 4（上限 8），深度耗尽时保留最内层归档 |
| 逐层多密码重试 | 每层都按完整候选池重试（外层和内层密码不同也能命中）；全部失败才报"需要密码" |
| 分段压缩包 | `.7z.001`/`.zip.001`/`.part1.rar` 同族卷自动聚合为一个任务（GUI 队列中同族卷行归并为一行）；只给非首卷会明确提示补第一卷；`.enc` 分卷逐卷解密后再按 7-Zip 分卷解 |
| Windows 单 exe | 0.3.1 起 GUI/CLI 内嵌 7-Zip 25.01 与前端资源，首次运行解压到 `%LOCALAPPDATA%\Unparanoia\embedded-7z\25.01-r1\`（可用 `UNPARANOIA_7Z_CACHE_DIR` 覆盖）；v0.3.2 起 7z 子进程不弹控制台窗口 |
| 许可证查看 | `unparanoia-cli --licenses`、GUI 设置页「7-Zip 许可证」 |
| 密码库（密码管理页） | 多密码明文保存在本机 `config.json`（0600），GUI「密码」页增删/清空、可切换显示掩码 |
| 明暗主题 | 设置页可选 跟随系统 / 浅色 / 深色，跟随系统时监听系统配色并实时切换 |
| 智能输出布局 | 单条目直接摊开、多条目自动建文件夹（smart/folder/direct） |
| 冲突处理 | rename（默认，`hello (1).txt`，沿用旧 paranoia 风格；upzip 基线是 `hello_1.txt`）/ skip / overwrite；目录只合并不清空 |
| 垃圾条目清洗 | 默认删除 `__MACOSX`、`._*`、`.DS_Store`、`Thumbs.db` 等 |
| 断点式重跑 | `processed.json` 记录已成功输入；**v0.3.2 起默认关闭**，仅 `--skip-processed` / GUI 设置开启时按指纹跳过重跑（旧配置一次性迁移为关闭） |
| 口令候选 | CLI `-p` → 环境变量 `UNPARANOIA_PASSWORD` → 密码库全部 → 压缩包注释提示 → 空密码（可选）；每个候选都会逐层尝试 |
| 记录/清理 | `cleanup-staging` 清理临时残留；GUI 设置页有同名按钮 |

**不做**：加密/打容器、Paranoia v3 容器、PFTEREC1 恢复容器、自动更新、
安装器（InstallShield/Inno/NSIS）内嵌文件的暴力解包、联网/云同步。

## 快速开始（GUI）

```bash
pnpm --dir gui install
pnpm --dir gui tauri dev      # 开发模式
# 或构建正式应用：
./scripts/build.sh            # -> target/release/bundle/{macos,dmg}
```

把 `.enc` 或压缩包拖进窗口 → 输入口令（可勾选「记住到密码库」）→ 开始。
密码库在「密码」页管理；外观主题在「设置」页切换。处理事件通过
`unparanoia://event` 推送，结束后 `unparanoia://done` 汇总。

排查问题时：日志在 `<configDir>/logs/unparanoia.log`（启动、7-Zip 定位、任务关键行、
错误与 panic 都会写；>2 MiB 轮转）；设置页「诊断信息」可复制版本/路径/7-Zip 来源/
配置摘要。v0.3.2 起 GUI 不再弹黑色控制台窗口（7z 子进程 `CREATE_NO_WINDOW`）。

## 快速开始（CLI）

```bash
cargo build --release -p unparanoia-cli
CLI=target/release/unparanoia-cli

$CLI doctor                                  # 7-Zip 路径/版本、配置目录、自检
$CLI detect 文件.enc                          # 只识别，不写盘
$CLI inspect 文件.enc                         # 打印 SSEFE 头（版本/算法/KDF 参数/salt）
$CLI run 文件.enc -p 口令 -o 输出目录           # 解密 + 解内层
$CLI run 一批文件... -p 口令 --jobs 4 --layout smart --conflict rename --json
$CLI run 3138.7z.001.enc 3138.7z.002.enc -p 口令 -p 内层口令   # .enc 分卷组自动聚合
$CLI run 包.7z.001 -p 口令                    # 普通分卷：同目录其余卷由 7-Zip 自动读取
$CLI run 文件.enc -p 口令 --skip-processed    # 显式启用「跳过已处理」（默认不跳过）
$CLI --licenses                               # 查看内嵌/随包 7-Zip 许可证

# 密码库（config.json 中的多密码，明文保存）
$CLI config --list-passwords                 # 列出密码库（默认掩码，见 --show 说明）
$CLI config --add-password 口令
$CLI config --remove-password 0              # 0 基索引
$CLI config --clear-passwords
$CLI config --show                           # 配置路径 + 密码库状态

$CLI cleanup-staging --root ~/Downloads      # 清理 .unparanoia-tmp-* 残留
```

常用选项：`-p/--password`（可重复）、`-o/--output-dir`、`-j/--jobs`、
`--conflict rename|skip|overwrite`、`--layout smart|folder|direct`、
`--no-recursive`、`--no-nested`、`--max-depth 1-8`、`--keep-broken`、`--dry-run`、
`--force-documents`、`--stop-on-error`、`--skip-processed`（默认不跳过；旧
`--no-skip-processed` 仍接受但为兼容 no-op）、`--no-strip-junk`、
`--try-empty-password`、`--json`、`--sevenzip PATH`。

退出码：`0` 成功/跳过，`1` 有失败或需要密码，`2` 用法/配置/环境错误，`130` 被 Ctrl-C 取消。
`--json` 输出纯 JSON Lines：若干 `TaskEvent`，最后一行 `{"type":"summary",...}`。
密码来源优先级：`-p`（可重复）→ `UNPARANOIA_PASSWORD`（兼容 `PARANOIA_ENC_PASSWORD`）→
密码库全部 → 引擎内部的归档注释提示 / 空密码（`--try-empty-password`）。

## 安全模型（重要）

- 解密在内存/临时文件中完成：SSEFE 明文先写入临时 ZIP（`TempGuard` 保证退出时删除），
  再解包到 staging，最后原子搬运到输出目录；失败默认清 staging（`--keep-broken` 可保留）。
- **不执行**被解出的文件；自解压 EXE 的"执行回退"默认关闭且仅 Windows 可选
  （`--allow-execute-sfx`，本项目未验证过该路径）。
- 口令存储：勾选「记住到密码库」或使用 `config --add-password` 时，口令以**明文**写入
  `config.json`（**明文**，文件权限 0600，仅 Unix 生效；Windows 无此权限位）。这是便利性设计，
  不是加密保险箱；共享电脑上请不要保存敏感口令。设置 `UNPARANOIA_CONFIG_DIR` 可改变配置目录
  （兼容读取旧环境变量 `PARANOIA_ENC_CONFIG_DIR`）。
- 从 v0.2.0 升级：如果旧配置目录 `paranoia-enc-decryptor` 里已有口令，首次运行会**只读迁移**
  到新目录 `unparanoia`（旧文件不删除）。
- 口令候选会通过 `7z` 的 argv 传入，同机其他进程理论上可从进程表读取
  （Windows 上尤其如此；本项目未实现更隐蔽的传递方式），属于已知限制。
- 应用**代码不发起网络请求**（本项目直接依赖里没有 HTTP client）；CSP 限制为
  `default-src 'self'`，不允许远程脚本/连接。注意 Tauri 框架的依赖树中仍可能间接
  出现 `reqwest`/`hyper`（被禁用/未调用），`cargo tree -i reqwest` 可核查。
- 7-Zip 以子进程运行，输入文件只读；恶意压缩包仍可能触发 7-Zip 自身漏洞——
  请保持 7-Zip 版本更新（本仓库固定 25.01，升级方法见 `binaries/README.md`）。
- Windows 单 exe 内嵌 7-Zip 二进制：首次运行会解压到用户缓存目录，之后复用；
  内嵌字节与 `binaries/win/` 完全一致，许可证可随时用 `--licenses` 查看。

## 配置与数据位置

| 文件 | 内容 | macOS | Windows |
|---|---|---|---|
| `config.json` | 密码库（多密码，明文，0600） | `~/Library/Application Support/unparanoia/` | `%APPDATA%\unparanoia\` |
| `settings.json` | 设置（含 `theme`、跳过已处理开关；v0.3.2 起含 `settingsVersion`） | 同上 | 同上 |
| `processed.json` | 处理记录（断点跳过）；**默认不生成**，仅开启跳过并成功后写入 | 同上 | 同上 |
| `logs/unparanoia.log` | GUI/CLI 日志（>2 MiB 轮转为 `.1`） | 同上 | 同上 |

CLI 的 `config --path` / `doctor` 会打印实际路径。旧目录 `paranoia-enc-decryptor/`
仅作为迁移/回退读取来源，新写入一律进入 `unparanoia/`。

## 输出布局（摘要）

- 默认 smart：解出 1 个顶层条目 → 直接摊在输出目录；≥2 个 → 建 `<归档名>/`。
- `<归档名>` 去掉一个扩展名（`.enc`/`.zip`/`.7z`…）并清洗非法字符。
- 分段压缩包：`3138.7z.001` + `3138.7z.002`（或对应 `.enc`）聚合为**一个任务**，
  输出到 `3138/`；只给非首卷时跳过并提示「请添加第一卷」，缺后续卷时明确报错且不留半成品。
- 冲突默认 rename：`hello.txt` → `hello (1).txt`（沿用旧版 paranoia 的 `(n)` 风格；
  upzip 基线是 `hello_1.txt`，合并时选了前者）；目录冲突整个改名，不与旧目录交叉合并。
- 临时目录/残留/失败策略详见 [docs/OUTPUT-LAYOUT.md](docs/OUTPUT-LAYOUT.md)。

## 构建

```bash
./scripts/build.sh                                   # macOS：测试 + 前端 + tauri 打包
./scripts/build-windows-portable.sh                  # macOS 交叉编译 Windows 便携 zip
.\scripts\build-windows.ps1                           # 真实 Windows：NSIS 安装包（未验证）
```

依赖与产物细节见 [docs/BUILD.md](docs/BUILD.md) 和
[docs/WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md)。注意本仓库是单一 Cargo workspace，
产物在**仓库根** `target/release/`。v0.3.3 最终构建的产物体积/SHA-256 见
[docs/VERIFICATION.md](docs/VERIFICATION.md) §7（每次重建会变）。

## 验证（诚实版）

- `cargo test --workspace`：**114 通过 / 0 失败 / 2 ignored**（core 43 + oracle 向量 1 +
  embedded-7z 3 + unpack-core 65 + GUI lib 2；2 个 ignored 的测试需要真实样本）。
- `./scripts/e2e-verify.sh`：**104 项 CLI e2e 全部通过**（合成 fixture，覆盖 SSEFE、
  加密 7z、前缀 ZIP、级联、逐层多密码、密码库、冲突、JSON、错误口令、清理、
  普通分卷 / `.enc` 分卷 / 缺卷 / 只给非首卷，以及 v0.3.2 的「跳过默认关闭 /
  `--skip-processed` / `related_paths` 卷归并」）。
- 分段压缩包另在真实样本 `3138.7z.001.enc`+`.002.enc`（Paranoia 口令 123、内层 7z
  口令 anzixin）上由 Lead 手工复验：合并 1 个任务、输出 `3138/`（5 文件，352.85 MiB），
  只给首卷自动发现、只给非首卷跳过、缺卷失败，详见 VERIFICATION.md §8。
- v0.3.2 跳过行为回归（Lead 手工，release CLI）：两个 `.enc` 合并 2.2 s 成功、默认
  第二次运行仍成功（不跳过）、显式 `--skip-processed` 第二次 skipped 2 并生成
  `processed.json`；GUI 启动后日志实际落盘。详见 VERIFICATION.md §7/§8。
- v0.3.3 两处 GUI 修复的量化验证（Swift + WKWebView 最小页面，本机）：旧单行布局下活动
  文本长短使主进度条宽度 1022 → 714.73 px（被挤短 307.27 px），新两行布局下恒为
  1080 px；`-webkit-text-security: disc` 在 WKWebView 计算值为 `disc`、`CSS.supports`
  为 true 且密码渲染为圆点；`pnpm --dir gui build` exit 0。Windows 真机（WebView2 +
  中文输入法）行为仍待用户验证，见 VERIFICATION.md §10。
- 真实样本 `3138.7z.001.enc`/`.002.enc`（密码 123）曾在 2026-10-23 通过**独立 Java
  Oracle** 全链路验证（MAC/checkcode/ZIP CRC/明文 SHA-256/`cmp` 全部一致）。
  但该验证**当前不可重跑**（Oracle 前置与原始明文文件已不在本机），仓库里只有
  可离线回放的 `verification/oracle/test-vectors.json`。
- **未验证**：真实 Windows 运行（含无控制台窗口、单 exe 首次解压缓存）、GUI 手工交互
  （密码页/主题切换/诊断按钮）、DMG 签名/公证、CI 实跑。逐项清单见
  [docs/VERIFICATION.md](docs/VERIFICATION.md) §10。

## 目录结构

```text
crates/core/            Paranoia 格式、KDF、AES-CTR、BLAKE3 MAC、SSEFE 解密、内层 ZIP 解压、密码库
crates/unpack-core/     检测、7-Zip 封装、布局/冲突、队列、嵌套、逐层多密码、分段压缩包、settings/processed、SSEFE 探测
crates/embedded-7z/     Windows 单 exe 内嵌的 7-Zip 25.01（ensure_7z/离线缓存/许可证文本）
crates/cli/             unparanoia-cli（detect/inspect/run/config/doctor/cleanup-staging）
gui/                    React + Vite 前端（队列/日志/密码/设置）
gui/src-tauri/          Tauri v2 后端（命令/事件/打包配置）
binaries/               随附 7-Zip 25.01（macOS 7zz + Windows 7z.exe/7z.dll + 许可证）
assets/                 品牌图标源文件（icon.svg）
scripts/                build / sync-binaries / make-fixtures / e2e-verify / Windows 便携打包
verification/oracle/    独立 Java Oracle 与已提交的测试向量
docs/                   FORMAT / CORE_API / OUTPUT-LAYOUT / BUILD / WINDOWS_BUILD / VERIFICATION / MERGE-MAPPING
```

## 文档

- [docs/FORMAT.md](docs/FORMAT.md) — Paranoia v4 文件格式与 KDF 管线
- [docs/CORE_API.md](docs/CORE_API.md) — Rust core API 契约（含密码库多密码 API）
- [docs/OUTPUT-LAYOUT.md](docs/OUTPUT-LAYOUT.md) — 输出布局/冲突/临时/失败语义/逐层密码候选
- [docs/BUILD.md](docs/BUILD.md) / [docs/WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md) — 构建
- [docs/VERIFICATION.md](docs/VERIFICATION.md) — 验证证据与未验证清单
- [docs/oracle-report.md](docs/oracle-report.md) — 真实样本 Oracle 验证历史记录
- [docs/MERGE-MAPPING.md](docs/MERGE-MAPPING.md) — 与两个前身的功能映射及 v0.3.0–v0.3.3 增量
- [docs/DEV-CONTRACT.md](docs/DEV-CONTRACT.md) — v0.3.0 内部接口契约（开发用）

## 版本与行为差异（有意）

| 版本 | 主要变化 |
|---|---|
| v0.1.x | 只解密 `.enc`；单密码；静态 HTML 前端 |
| v0.2.0 | 合并 unparanoia + upzip：批量解压、伪装压缩包、级联、并行、settings/processed；单密码明文记忆 |
| v0.3.0 | 更名 **Unparanoia（帕罗诺亚解包器）**；密码库（多密码明文）+ 密码管理页；逐层多密码重试；明暗主题；新图标；配置/env/事件更名（旧名兼容读取） |
| v0.3.1 | 分段压缩包（普通卷 + `.enc` 分卷）；Windows 改为**单 exe**（内嵌 7-Zip 与前端）；修复 Windows GUI 白屏（`--features custom-protocol`）；`--licenses`/GUI 许可证查看；版本号 0.3.1 |
| v0.3.2 | 跳过已处理**默认关闭**（旧配置一次性迁移，CLI `--skip-processed` opt-in）；分卷在队列中行归并（`TaskOutcome.related_paths`）/全跳过提示条；Windows 无控制台闪窗 + 7z 错误细节 + 锁文件重试；日志落盘/设置页诊断（复制/打开日志/清除处理记录）；版本号 0.3.2 |
| v0.3.3 | GUI 修复：主进度条不再被右侧活动文本（当前文件/输出路径）挤得忽长忽短——进度条独占一行、活动文本另起一行截断；密码输入框改用 `-webkit-text-security: disc` 掩码并回退 `type=password`，支持用输入法直接键入中文密码；版本号 0.3.3 |

升级注意：

| 项 | v0.2.0 | v0.3.0 |
|---|---|---|
| 产品名 / 可执行文件 | Paranoia-Decryptor / paranoia-enc-decryptor | **Unparanoia** / `unparanoia`（CLI `unparanoia-cli`） |
| 口令存储 | 单个「记住的口令」 | 密码库（多密码数组；旧单值自动迁移） |
| 事件 | `paranoia://event` / `paranoia://done` | `unparanoia://event` / `unparanoia://done` |
| 环境变量 | `PARANOIA_ENC_*` | `UNPARANOIA_*`（旧名仅回退读取；`UPZIP_7Z` 仍兼容） |
| 嵌套密码 | 每层单独试单个密码，失败即放弃 | 每层重走完整候选池，全部失败才报需要密码 |
| 冲突重命名 | `hello (1).txt` | 不变 |
| 范围 | 解密 `.enc` + 批量解压 | 不变（+ 密码库/主题/分卷） |

v0.3.3 变更：仅 GUI 前端（`gui/src/App.tsx`、`gui/src/App.css`），Rust/CLI/引擎无改动。
主进度条改为独占一行（宽度只随窗口变化），活动文本另起一行省略，不再出现进度条随
文件名/路径长短伸缩；密码输入框在支持的 WebView（WKWebView / WebView2）上使用
`type="text"` + `-webkit-text-security: disc` 掩码，使中文输入法可直接键入（不再只能
粘贴），不支持时自动回退 `type="password"`。密码仍为明文保存（仅存本机）。

v0.3.2 变更：**跳过已处理默认关闭**（旧 `settings.json` 一次性迁移为关闭），
默认重复运行会重新解压；要跳过需 `--skip-processed` 或 GUI 设置开启。Windows GUI
不再弹控制台窗口，日志/诊断入口见「安全模型」与「配置与数据位置」。

## 许可证

本项目代码：MIT，见 [LICENSE](LICENSE)。随附 7-Zip 25.01：LGPL v2.1+（含 unRAR 限制），
见 [NOTICE](NOTICE) 与 [binaries/README.md](binaries/README.md)。

## 已知限制

- 只支持 Paranoia **v4**（SSEFE）；v3 与 PFTEREC1 恢复容器明确不支持。
- 加密头部的归档在无口令时无法枚举内容（只能报告"需要密码"）。
- 7-Zip 25.01 对"前缀 ZIP"的拒绝已用切片兜底，但其它版本的 7-Zip 行为可能不同。
- 带口令的安装器（InstallShield/Inno/NSIS）内嵌文件不在支持范围。
- 未在真实 Windows 上运行验证；Windows 便携版仅交叉编译产出（静态证据见 VERIFICATION.md §9；v0.3.2 的 `CREATE_NO_WINDOW`/重试/错误细节也只有单测与静态证据）。v0.3.3 的进度条布局与密码框输入法修复虽用 WKWebView 量化验证，但 Windows WebView2 真机行为（尤其中文输入法）仍待用户复验。
- 分段压缩包必须从第一卷开始（`.001` / `.part1.rar`）；缺后续卷会明确报错，不会输出半成品。
- Windows 单 exe 首次运行需写入用户缓存目录（`%LOCALAPPDATA%\Unparanoia\`）。
- 密码库是明文存储（0600 仅 Unix 生效），不是加密保险箱。
- 不做自动更新、云同步、历史记录、硬暂停（暂停是协作式，进行中的 7-Zip 子进程会跑完当前任务段）。
