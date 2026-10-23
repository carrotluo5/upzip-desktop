# Paranoia Enc Decryptor

> 轻量级跨平台 GUI 解密工具，用于解密 Paranoia File Encryption v4（AES-256 口令模式）生成的 `.enc` 文件。
> 技术栈：Rust + Tauri v2 + 纯静态前端（HTML/CSS/JS），支持 macOS arm64 与 Windows x64。

---

## 项目简介

Paranoia Enc Decryptor 是 [Paranoia File Encryption](https://github.com/apronus-inc/Paranoia-File-Encryption)（v4，AES-256 / 口令模式）的**只读解密客户端**。它不重新发明密码学：格式解析、口令派生（Skein/HKDF + Argon2id）、AES-256-CTR 解密、BLAKE3 完整性校验和内嵌 ZIP 解包全部集中在独立、可测试的 Rust core crate（`paranoia-enc-core`）中，GUI 只负责交互与展示。

设计目标：

- **轻量**：安装包为个位数 MB 级，启动接近瞬时，不携带 Chromium/Node 运行时；
- **跨平台**：macOS（Apple Silicon 原生 arm64）与 Windows x64 共用同一套 core 逻辑；
- **可验证**：每个字节都有对照——用真实样本做端到端 SHA-256 校验（见[样本验证方法](#样本验证方法)）；
- **克制**：一期只做「解密 AES-256 `.enc` 文件」这一件事，其余需求明确列为不支持或后续扩展点。

当前版本：`0.1.1`（Phase 1）。

## 界面预览

![Paranoia 解密工具界面](docs/screenshots/gui-main.png)

> 实际运行截图（macOS arm64）。密码记忆勾选后会以明文写入用户配置目录，界面会直接显示该文件路径。

## 技术栈与选型理由

| 层 | 选型 | 说明 |
|---|---|---|
| 核心逻辑 | Rust（`paranoia-enc-core`） | 纯同步、无 async/无网络/无 GUI 依赖；密码学库为纯 Rust 实现，便于审计与跨平台复用 |
| 桌面外壳 | Tauri v2 | 使用系统自带 WebView（macOS WKWebView / Windows WebView2），Rust 进程为唯一后端 |
| 前端 | 纯静态 HTML + CSS + JS | 无前端框架、无打包器；`tauri build` 直接内嵌静态资源，构建链最短 |
| 命令行 | Rust（`paranoia-enc-cli`） | 无头验证 / 批量解密 / CI 冒烟测试用，不属于一期交付的用户界面 |

### 为什么是 Rust + Tauri v2，而不是 Electron

| 维度 | Tauri v2（本项目） | Electron |
|---|---|---|
| 安装包体积 | 约 2–10 MB（本项目实测 DMG 4.25 MiB / `.app` 5.8 MiB，不含系统 WebView） | 通常 80–150 MB（自带 Chromium + Node.js） |
| 冷启动 | 通常 < 1 s（Rust 进程 + 系统 WebView；本机实测见 docs/VERIFICATION.md） | 通常 1–3 s（加载 Chromium/Node 运行时） |
| Apple Silicon | 原生 arm64 二进制，无 Rosetta | 需分发 arm64 版本；Electron 本体仍较重 |
| 内存占用 | 与页面复杂度同阶，无独立 JS 运行时进程树 | 主进程 + GPU + 多个渲染进程 |
| 攻击面 | 默认仅暴露显式注册的 command；无 Node 集成 | 主进程拥有完整 Node 能力，需额外加固 |
| 密码学核心复用 | 直接链接 Rust crate，CLI/GUI 共享同一份实现 | 需通过 native addon 桥接，边界更脆 |

对本项目而言，GUI 只是「选文件 → 输密码 → 看进度 → 看结果」，页面极简；Electron 的体积与启动成本无法用其生态优势抵消。Tauri v2 还提供各平台原生文件对话框、拖放事件与事件推送进度，满足一期全部交互需求。

## 功能特性

- **添加文件**：拖放 `.enc` 文件到窗口，或点击按钮通过系统对话框多选添加；支持一次添加多个文件。
- **文件列表管理**：去重、移除单个文件、清空列表；添加时即校验扩展名与文件头，不合格的文件会被拒绝加入并提示具体原因（不会进入列表）。
- **密码输入与记忆**：口令输入框 + 「记住密码」勾选；勾选后密码以**明文**写入 OS 用户配置目录（见[密码记忆](#密码记忆存储位置与明文风险)），下次启动自动填充；提供「清除记忆」按钮。
- **输出目录**：默认 = 列表第一个文件所在目录；可手动指定自定义目录。
- **冲突策略**：目标文件已存在时可选 `覆盖` / `重命名` / `直接报错`（CLI 默认 `rename`）。
- **进度反馈**：每个文件独立进度条（按字节）+ 整体进度条（按已处理字节合计）；底部成功/失败/总数按文件计数，解密过程中界面保持响应。
- **可读错误**：区分「口令错误 / 文件损坏或篡改 / 不是 Paranoia 文件 / 版本或算法不支持 / 输出冲突 / 权限或 IO 错误」等，并给出中文提示；单文件失败不影响批次内其他文件。
- **安全默认值**：不联网、不上传任何数据、不写日志到网络位置；输出目录可自定义，明文文件仅写入用户指定位置。

## 支持范围与明确不支持项

### 支持（Phase 1）

| 能力 | 说明 |
|---|---|
| 文件格式 | Paranoia File Encryption **v4**（`SSEFE` 魔数 + 版本 `0x04`） |
| 加密算法 | **仅 AES-256**（算法字节 `0x00`，AES-256-CTR） |
| 口令模式 | 单一口令解密（含 Paranoia 的字符规整规则） |
| 密钥派生 | Skein-1024 HKDF（l0 口令哈希）+ Argon2id（参数由头部 `customParams` 编码） |
| 完整性 | BLAKE3 keyed MAC 全量校验 |
| 原始文件名 | 从内嵌 ZIP 条目恢复原始文件名/目录结构（ZIP 可为 Zip64；方法 STORED / DEFLATED / ZSTD） |
| 平台 | macOS arm64（本地构建验证；GUI 运行验证见 [docs/VERIFICATION.md](docs/VERIFICATION.md)）；Windows x64（CI 构建 + 本机交叉编译，未在真实 Windows 上运行，见 [docs/WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md)） |

### 明确不支持（一期不做）

- ❌ **加密**（本工具只解密，不产生 `.enc` 文件）；
- ❌ AES-128 / 其他算法字节、未来版本（非 v4）格式；
- ❌ PFTEREC1 恢复容器（检测到即明确报错 `unsupportedRecovery`，不做恢复尝试）；
- ❌ 任务队列、断点续传、暂停/恢复、后台批处理调度；
- ❌ 云同步、网络传输、遥测；
- ❌ 主题/皮肤、多语言界面（一期仅中文界面）；
- ❌ 密钥文件模式 / 硬件密钥 / 系统钥匙串集成（密码记忆为明文文件）。

> 若文件来自更高版本或非 AES-256 算法，程序会以「不支持的版本/算法」错误中止，而不会尝试猜测解密。

## 文件格式说明

完整格式、KDF 细节与错误判定顺序见 **[docs/FORMAT.md](docs/FORMAT.md)**（基于样本逆向 + 官方源码交叉确认）。核心摘要：

**整体布局**

```text
[0, 8)        明文头 8B: "SSEFE"(5B) | version(1B=0x04) | algorithm(1B=0x00 AES-256) | customParams(1B)
[8, 40)       salt: 32B 随机盐
[40, L-32)    ciphertext: AES-256-CTR 加密的明文流
明文流        = checkcode(32B, [a-z0-9] 随机字符) || 内嵌 ZIP 归档
[L-32, L)     MAC: BLAKE3 keyed（32B），覆盖 nonce||header||salt||ciphertext
```

- **KDF（v4）**：口令规整（可打印 ASCII 保留，其余字符替换为十进制码点串）→ Skein-1024 HKDF 得到 256B `l0PWHash` → Argon2id（`t=10*2^tm`、`m=10240*2^mm KiB`、`p=4`、输出 256B）→ 再用 HKDF-SHA3-512 派生 `encKey(32B)` / `authKey(32B)` / `nonce(16B)`。
- **加密**：AES-256-CTR，初始计数器 = 16B nonce，按 16B 块 +1，无填充。
- **口令快速判错**：明文流前 32B 的 checkcode 必须全部属于 `[a-z0-9]`，否则判为口令错误。
- **完整性**：BLAKE3 keyed MAC（key = `authKey`）覆盖 `nonce || header || salt || ciphertext` 全量密文；不匹配 → 文件损坏/被篡改（或口令错误）。
- **原始文件名恢复**：去掉 32B checkcode 后的明文流是一个标准 ZIP；单文件加密时 ZIP 仅含一个 entry，entry 名即原始文件名（UTF-8，Unix 分隔符）；目录加密时按 entry 相对路径还原目录树。每个 entry 带 CRC32，解压时校验。

**错误判定顺序**（实现约定）：长度过短 → 魔数 → 版本 → 算法 → 参数编码 → 恢复容器魔数 → checkcode（口令） → MAC（完整性） → ZIP CRC。

## 构建与运行

### 环境依赖

| 工具 | 版本要求 | macOS | Windows |
|---|---|---|---|
| Rust | stable（本项目在 rustc 1.99.0 上验证） | `rustup` 安装 | `rustup` 安装（MSVC toolchain） |
| Node.js | ≥ 20（本项目用 26.x） | `brew install node` 或 nvm | 官网安装包或 nvm-windows |
| pnpm | ≥ 9（本项目用 11.x） | `corepack enable` 或 `npm i -g pnpm` | 同左 |
| 系统构建依赖 | — | Xcode Command Line Tools：`xcode-select --install` | Visual Studio Build Tools（C++ 桌面开发） |
| 系统 WebView | — | 系统自带 WKWebView | WebView2 Runtime（Windows 10/11 通常已预装） |

> Linux 不是一期目标平台，本环境也未验证。`crates/core` 与 `crates/cli` 仅依赖纯 Rust crate，理论上可移植；GUI 需额外安装 Tauri 的 Linux 系统依赖（`libwebkit2gtk-4.1-dev` 等）。

### macOS arm64（本地已验证）

```bash
# 1. 一键构建（推荐）：跑测试 → 装 GUI 依赖 → tauri build → 打印产物
./scripts/build.sh

# 2. 手动等价流程
cargo test --workspace
cd gui && pnpm install && pnpm tauri build
```

产物（`productName = Paranoia Decryptor`，`identifier = com.nanase.paranoia-decryptor`，`bundle.targets = all`）：

- `gui/src-tauri/target/release/bundle/macos/Paranoia Decryptor.app`（应用本体）
- `gui/src-tauri/target/release/bundle/dmg/Paranoia Decryptor_0.1.1_aarch64.dmg`（安装镜像，文件名随版本/架构变化）
- `gui/src-tauri/target/release/paranoia-enc-decryptor`（裸二进制，便于直接运行）

本机实测（2026-10-24，macOS 27.2 arm64，v0.1.1）：GUI release 增量构建 29.80 s；`.app` 5.8 MiB、`.dmg` 4.25 MiB（4,457,433 B）、裸二进制 4,888,448 B。测试 35 个 core 单元测试 + 1 个 oracle 向量测试通过，2 个真实样本测试默认 ignored；完整构建与验证记录见 [docs/VERIFICATION.md](docs/VERIFICATION.md)。

首次打开未签名应用时，macOS Gatekeeper 可能拦截：右键 App →「打开」，或 `System Settings → Privacy & Security → Open Anyway`。

### Windows x64

```powershell
# 一键构建（PowerShell，推荐）
powershell -ExecutionPolicy Bypass -File scripts/build-windows.ps1

# 手动等价流程
cargo test --workspace
cd gui
pnpm install
pnpm tauri build
```

产物：`gui\src-tauri\target\release\bundle\nsis\*-setup.exe`（NSIS 安装包，可用 `pnpm tauri build --bundles nsis` 只构建安装包）与 `gui\src-tauri\target\release\paranoia-enc-decryptor.exe`。未签名安装包会触发 SmartScreen 提示，需「更多信息 → 仍要运行」。

**便携版单文件 exe（可选）**：在 macOS 上可用 `./scripts/build-windows-portable.sh`（底层为 `cargo-xwin`）交叉编译出**单文件**便携版 `paranoia-enc-decryptor.exe`（v0.1.1 实测 6,980,096 B，PE32+ x86-64 / Windows GUI 子系统）。Rust 后端与静态前端资源全部打进一个 exe，**不需要随附 DLL、目录或安装程序**；运行时仅依赖 Windows 系统组件：UCRT（Windows 10+ 自带）与 **WebView2 Runtime**（Windows 11 自带，Windows 10 通常随 Edge/系统更新安装；缺失时从微软官网安装 Evergreen Runtime）。导入表中没有 `VCRUNTIME140.dll`、也没有 `WebView2Loader.dll`。注意：这是**交叉编译**产物，未在真实 Windows 上运行过（见下方诚实说明）。

> **诚实说明**：本仓库开发机为 macOS arm64，Windows 运行时行为**未在真实 Windows 机器上验证**（无 Windows PC/VM，也未用 Wine 运行）。
> Windows 侧证据：(1) `.github/workflows/build.yml` 的 `windows-latest` 任务（权威构建路径；仓库当前无 remote，尚未实际跑过）；(2) 本机 `cargo-xwin` 交叉编译成功的裸产物——CLI `paranoia-enc-cli.exe`（781,312 B）与 GUI 单文件便携版 `paranoia-enc-decryptor.exe`（v0.1.1：6,980,096 B，PE32+ / Subsystem GUI），但**均未在 Windows 上执行**，也未生成 NSIS 安装包。逐条命令、版本与错误见 **[docs/WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md)**。

### 开发模式

```bash
cargo test --workspace            # core + CLI 全部测试
cargo run -p paranoia-enc-cli -- --help
cd gui && pnpm install && pnpm tauri dev   # 启动 GUI 开发窗口（等价于 pnpm dev）
```

## 一键脚本说明

| 脚本 | 平台 | 行为 |
|---|---|---|
| `scripts/build.sh` | macOS / Linux | `set -euo pipefail`；检查 `cargo`/`node`/`pnpm`；`cargo test --workspace`；`cd gui && pnpm install`；`pnpm tauri build`；最后打印 bundle 产物的路径与大小 |
| `scripts/build-windows.ps1` | Windows | 检查 `cargo`/`node`/`pnpm`；`cargo test --workspace`；`cd gui`；`pnpm install`；`pnpm tauri build`（NSIS）；打印安装包路径与大小 |
| `scripts/build-windows-portable.sh` | macOS（交叉编译到 Windows） | 检查 `cargo-xwin` 与 rustup stable；`cargo xwin build --release --target x86_64-pc-windows-msvc`；打印单文件便携 exe 的路径、大小与 SHA-256 |
| `.github/workflows/build.yml` | CI | 矩阵 `macos-14`（arm64）+ `windows-latest`：checkout → setup node → pnpm → rust toolchain → `cargo test` → 构建 GUI → 上传产物 |

两个脚本都是**幂等**的：重复执行只会增量编译/安装依赖，不会清除已有产物。所有构建均从仓库根目录执行。

## 使用说明

1. **添加文件**：把 `.enc` 文件拖入窗口，或点击「添加文件」在系统对话框里多选（不支持整目录添加）。重复文件自动去重；添加时会立即校验文件头，非 Paranoia v4 / 非 `.enc` 文件会被拒绝并提示原因。
2. **输入密码**：在密码框输入口令。勾选「记住密码」后，密码会以明文保存到用户配置目录（见下节），下次启动自动填充；不勾选则仅在本次运行的内存中保留。
3. **选择输出目录**：默认输出到**列表第一个文件所在目录**；点击「选择…」可改为任意可写目录。
4. **选择冲突策略**：目标文件已存在时——`重命名`（默认，在最后一个扩展名前追加序号，如 `3138.7z.002` → `3138.7z (1).002`）/ `覆盖` / `直接报错`（错误码 `outputExists`）。
5. **开始解密**：点击「开始解密」，可看到当前文件进度与整体进度、成功/失败计数；单文件失败会记录原因并继续处理其余文件。
6. **查看结果**：完成后列出每个文件的输出路径；原始文件名/目录结构从内嵌 ZIP 恢复，扩展名与加密前一致。
7. **清除记忆**：点击界面上的「清除记忆」按钮，或执行 `cargo run -p paranoia-enc-cli -- --clear-password`，即可删除本地保存的明文密码（`--config-path` 可打印存储位置）。

## 样本验证方法

仓库不附带真实样本（避免提交用户数据）。可用手上的 Paranoia v4 样本按下述流程做端到端验证。本文档使用的两个样本：

| 样本文件 | 密文大小 | 密文 SHA-256（校验样本未改动） | 解密后明文大小 | 明文 SHA-256 |
|---|---|---|---|---|
| `3138.7z.001.enc` | 209,779,415 B | `078386d3ad550d438dcaa2f6297e5b75112e4fbaea6731685d56bc7e8254877c` | 209,715,200 B | `9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c` |
| `3138.7z.002.enc` | 60,527,110 B | `42ef9720fef2b13cea7548be26261ce0b29df07aaddeb06208a70fbf7c9c9623` | 60,508,410 B | `46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840` |

> 样本口令：`123`。macOS 上验证时把样本复制到 `/tmp`（避免直接读写外部卷），解密产物同样写到 `/tmp`。
>
> ✅ **实测记录**（macOS arm64，release CLI）：两个样本均被逐字节还原，SHA-256 与上表完全一致；错误口令返回 `wrongPassword` 且退出码 1，不产生输出文件。

CLI 选项（`cargo run --release -p paranoia-enc-cli -- --help` 实测）：

| 选项 | 说明 |
|---|---|
| `-p, --password <PASSWORD>` | 口令；也支持环境变量 `PARANOIA_ENC_PASSWORD`（便于脚本/CI） |
| `-o, --output-dir <DIR>` | 输出目录，缺省 = 第一个输入文件所在目录 |
| `--conflict <overwrite\|rename\|error>` | 冲突策略，默认 `rename` |
| `--json` | 输出机器可读 JSON（批量脚本用） |
| `--inspect` | 只打印头部信息，不解密 |
| `--config-path` | 打印密码存储文件路径 |
| `--clear-password` | 删除记住的密码 |

退出码：`0`=全部成功，`1`=存在失败文件，`2`=参数/IO 错误。

### 单文件验证（CLI）

```bash
# 0. 准备（以 3138.7z.002.enc 为例；密码 123）
cp /path/to/3138.7z.002.enc /tmp/sample.enc

# 1. 查看头部信息（不读全文件，不派生密钥）
cargo run --release -p paranoia-enc-cli -- --inspect /tmp/sample.enc

# 2. 解密到 /tmp/out
rm -rf /tmp/out && mkdir -p /tmp/out
cargo run --release -p paranoia-enc-cli -- -p 123 -o /tmp/out /tmp/sample.enc

# 3. 校验明文 SHA-256（对照上表）
shasum -a 256 /tmp/out/3138.7z.002
```

预期输出（仅单文件且 stderr 为终端时，进度百分比会就地刷新到 stderr；批量或重定向时只输出结果）：

```text
OK   /tmp/sample.enc -> /tmp/out/3138.7z.002 (original: 3138.7z.002, 60508410 bytes)
successes: 1, failures: 0, output dir: /tmp/out

46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840  /tmp/out/3138.7z.002
```

`--inspect` 预期输出（不派生密钥、不写输出）：

```text
file:           /tmp/sample.enc
size:           60527110 bytes
magic:          SSEFE
format version: 4
algorithm:      0 (AES-256)
custom params:  0x11 (Argon2id t=20, m=20480 KiB, p=4)
salt:           f0131e597c16317aeb749495f04593df2b0ac15a4e2f0768c7a493f7abe7e210
```

`3138.7z.001.enc` 同理，替换文件名并对照 `9d001d6c...`。

### 批量验证

```bash
cp /path/to/3138.7z.001.enc /tmp/s1.enc
cp /path/to/3138.7z.002.enc /tmp/s2.enc
rm -rf /tmp/out && mkdir -p /tmp/out
cargo run --release -p paranoia-enc-cli -- --json -p 123 -o /tmp/out /tmp/s1.enc /tmp/s2.enc
shasum -a 256 /tmp/out/3138.7z.001 /tmp/out/3138.7z.002
```

逐项对照上表 SHA-256；全部一致即通过。失败路径可用错密码（如 `-p wrong`）验证是否返回「口令错误」而不是崩溃。

## 密码记忆存储位置与明文风险

| 平台 | 路径 |
|---|---|
| macOS | `~/Library/Application Support/paranoia-enc-decryptor/config.json` |
| Windows | `%APPDATA%\paranoia-enc-decryptor\config.json` |
| Linux（仅 CLI） | `$XDG_CONFIG_HOME/paranoia-enc-decryptor/config.json`（缺省 `~/.config/...`） |

**风险说明（必读）**：

- 该文件保存的是**明文口令**，**没有加密**、没有混淆；任何能读取该用户配置目录的进程/用户都能直接看到密码。
- 程序**不会**把口令上传、同步或写入任何网络位置；只有勾选「记住密码」时才会落盘。
- 公共/共享电脑上**不要**勾选「记住密码」；使用后可通过界面「清除记忆」按钮或 CLI `--clear-password` 一键删除。
- 同期不实现系统钥匙串（macOS Keychain / Windows Credential Manager）集成，已在后续扩展点中列出。

## 测试

```bash
cargo test --workspace                 # 全部测试
cargo test -p paranoia-enc-core        # 仅 core 单元/集成测试
```

当前基线：**35 个 core 单元测试 + 1 个 oracle 向量测试（`crates/core/tests/oracle_vectors.rs`，与 Java 中间值逐项比对）全部通过**；另有 2 个真实样本集成测试（`crates/core/tests/real_sample.rs`）默认 `#[ignore]`，需设置 `PARANOIA_SAMPLE_ENC` / `PARANOIA_SAMPLE_SHA256` 环境变量并提供本地样本后启用。完整验收记录（E2E 哈希、批量、GUI、构建）见 **[docs/VERIFICATION.md](docs/VERIFICATION.md)**；独立 Java oracle 报告见 [docs/oracle-report.md](docs/oracle-report.md)。

`paranoia-enc-core` 测试覆盖：

- 头部解析：魔数、版本、算法、`customParams` 参数解码、长度不足/截断；
- KDF：口令规整规则、Skein-1024 l0 哈希、Argon2id 派生结果确定性（对照已知向量）；
- 解密：生成夹具端到端解密、checkcode 口令判错、MAC 篡改检测、CTR 计数器跨块正确性；
- 内嵌 ZIP：原始文件名恢复、目录归档还原、CRC 校验、zip-slip 路径清理；
- 输出与冲突：默认输出目录、自定义目录、`overwrite`/`rename`/`error` 三种策略；
- 密码记忆：保存/读取/清除、配置路径解析、损坏配置文件容错；
- 批量：失败隔离（一个坏文件不影响其余）、成功/失败计数、总体进度回调单调递增。

CLI 冒烟测试：`cargo run -p paranoia-enc-cli -- --help`、`--inspect`、真实样本解密 + SHA-256 对照。真实样本验证属于**手动验收**（样本不入库），本仓库开发机已完成两个样本的逐字节校验（见[样本验证方法](#样本验证方法)与 docs/VERIFICATION.md）。独立 Java oracle（`bash verification/oracle/test.sh`）不参考 Rust 实现，交叉复核 MAC、checkcode、CRC 与逐字节一致性。

### 真实样本集成测试（默认忽略）

```bash
PARANOIA_SAMPLE_ENC=/tmp/sample002.enc \
PARANOIA_SAMPLE_SHA256=46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840 \
cargo test -p paranoia-enc-core --test real_sample -- --ignored --nocapture
```

## 项目结构

```text
paranoia-enc-decryptor/
├── Cargo.toml                  # Rust workspace（core + cli）
├── crates/
│   ├── core/                   # paranoia-enc-core：格式/ KDF / AES-CTR / BLAKE3 / ZIP / 配置
│   └── cli/                    # paranoia-enc-cli：无头验证与批量解密 CLI
├── gui/
│   ├── src/                    # 纯静态前端（index.html / main.js / styles.css）
│   └── src-tauri/              # Tauri v2 应用（Rust commands + tauri.conf.json）
├── scripts/
│   ├── build.sh                  # macOS/Linux 一键构建
│   ├── build-windows.ps1         # Windows 一键构建（NSIS）
│   └── build-windows-portable.sh # macOS 上交叉编译 Windows 单文件便携 exe
├── verification/               # 独立 Java oracle：格式/KDF/MAC/解密交叉验证
├── docs/
│   ├── FORMAT.md               # 文件格式规范（逆向 + 源码交叉确认）
│   ├── CORE_API.md             # core crate 与 GUI/CLI 的接口契约
│   ├── VERIFICATION.md         # 验收记录：E2E 哈希、批量验证、GUI、构建/运行记录
│   ├── oracle-report.md        # 独立 Java oracle 交叉验证完整报告
│   ├── BUILD.md                # 构建细节与故障排查
│   └── WINDOWS_BUILD.md        # Windows x64 构建证据与交叉编译调查
├── .github/workflows/build.yml # CI：macos-14 + windows-latest 矩阵构建
└── README.md
```

## 已知限制与后续扩展点

### 已知限制

- 仅支持 Paranoia v4 + AES-256；其他版本/算法明确报错，不做兼容猜测。
- 不支持 PFTEREC1 恢复容器（存在该扩展的文件会报 `unsupportedRecovery`）。
- 密码记忆为明文文件，未接入系统钥匙串；共享设备上请勿勾选。
- 解密为单线程顺序批处理：Argon2id 参数（默认 `t=20, m=20 MiB, p=4`）使得**每个文件**都有固定的 KDF 开销，大文件解密时间主要受 KDF + IO 限制；暂无取消/暂停能力。
- 构建产物未做代码签名/公证：macOS 需右键打开或手动放行，Windows 会触发 SmartScreen。
- Windows x64 在本仓库开发机（macOS arm64）上**只做交叉编译**（CLI 裸 exe + GUI 单文件便携 exe，均未在 Windows 上运行）与 CI 工作流配置，未做 Windows 真机运行时验证；详见 [docs/WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md)。

### 后续扩展点

- 加密支持（对齐 Paranoia 官方 v4 加密输出，含随机盐/nonce 与参数选择）；
- 支持更多算法字节与未来格式版本；
- PFTEREC1 恢复容器的识别/恢复；
- 任务队列、断点续传、取消/暂停与并发解密；
- 系统钥匙串集成（macOS Keychain / Windows Credential Manager）；
- macOS 公证 + Windows 代码签名，以及自动更新。

## 版本记录

| 版本 | 日期 | 变更 |
|---|---|---|
| `0.1.1` | 2026-10-24 | 修复批量解密过程中个别文件行会**短暂误显示「失败」**、随后又变为「成功」的进度事件时序问题（根因：core 在文件最后一块数据回调时携带的仍是上一轮成功/失败计数，GUI 由该回调推导的临时结果被误判为失败；现在仅在实际计数变化时才发出临时结果事件）；新增应用图标（macOS `.icns` / Windows `.ico`）；提供 Windows 单文件便携版 exe 的交叉构建脚本；重新构建 macOS arm64 DMG。 |
| `0.1.0` | 2026-10-23 | 首个版本：Paranoia v4 / AES-256 `.enc` 解密核心、CLI、Tauri GUI、独立 Java oracle 与完整验证文档。 |

## 许可证

MIT（见 `Cargo.toml` 的 `workspace.package.license`）。
