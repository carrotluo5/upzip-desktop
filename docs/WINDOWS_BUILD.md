# Windows x64 构建与验证记录（WINDOWS_BUILD）

> 记录日期：2026-10-23（Asia/Shanghai）
> 记录机器：macOS arm64（Apple Silicon），**没有 Windows 机器、没有 Windows 虚拟机、没有 Wine**。

## 0. 结论摘要（先看这里）

| 项目 | 状态 | 证据 |
|---|---|---|
| Windows x64 **运行时验证**（真实 Windows 上运行 exe / GUI / 解密样本） | ❌ **未进行，也不可能在本机进行** | 本机无 Windows 环境；交叉编译产物**未被执行过**。本文档不伪造任何 Windows 运行结果。 |
| Windows x64 **CLI/core 构建** | ✅ 交叉编译成功（macOS 上 `cargo-xwin`） | 产出 `paranoia-enc-cli.exe`（PE32+ x86-64，781,312 B），详见 §4 |
| Windows x64 **GUI（Tauri）裸 exe 构建** | ✅ 交叉编译成功（macOS 上 `cargo-xwin`，**仅裸 exe，非安装包**） | 产出 6,886,912 B 的 PE32+ GUI exe（EXIT=0），详见 §5；**该 exe 未在 Windows 上运行过**，Windows 安装包仍走 CI |
| Windows x64 **从零构建的权威路径** | ✅ GitHub Actions `windows-latest`（`build.yml`） | 见 §6；CI 在真实 Windows runner 上执行 `cargo test` + `pnpm tauri build --bundles nsis` |

**一句话**：本机能证明的是「CLI 与 GUI 的 Rust 侧代码和依赖都可以交叉编译成 Windows x64 可执行文件」，**不能**证明「它们在 Windows 上能运行、能解密、GUI 窗口能打开、安装包能安装」。后者必须依赖 CI 构建 + 真机验收（§7 提供清单）。

## 1. 本机环境（实测）

```text
macOS 27.2（Build 26B118），Apple Silicon / arm64
Xcode Command Line Tools: /Library/Developer/CommandLineTools（Apple clang 21.0.0）
Rust:
  - rustup 1.29.1，stable toolchain rustc 1.98.1（aarch64-apple-darwin，已安装 x86_64-pc-windows-msvc target）
  - Homebrew rust 1.99.0（/opt/homebrew/bin/rustc，sysroot 无 windows target）
Node.js v26.10.0 / pnpm 11.27.0
cargo-xwin 0.23.1（xwin subcommand）
Homebrew LLVM 23.1.3（提供 clang-cl 23.1.3）
Windows 环境: 无（无 Windows 机器 / 无 VM / 无 Wine）
```

## 2. 为什么没有「Windows 运行时验证」

本仓库开发机是 macOS arm64：

- 没有 Windows 物理机，也没有安装 Parallels / VMware / UTM 等 Windows 虚拟机；
- 没有 Wine（`which wine wine64` → not found），无法执行交叉编译出的 `.exe`；
- Tauri GUI 还需要 Windows WebView2 运行时与桌面会话，macOS 上无法模拟。

因此：**所有 Windows 运行行为（窗口能否打开、拖放是否生效、解密是否正确、安装包能否安装）均未验证**。任何声称「已在 Windows 验证」的说法都不成立。下文只记录构建层面的证据。

## 3. 工具链准备与已知坑

### 3.1 安装步骤（本机实际执行）

```bash
# LLVM（提供 clang-cl，供 cargo-xwin 作为 MSVC 兼容编译器）——本机已预装 brew llvm
brew install llvm

# cargo-xwin（MSVC CRT/SDK 由它自动下载并注入链接参数）
cargo install cargo-xwin

# rustup 的 Windows target（注意：必须是 rustup 的 toolchain）
rustup target add x86_64-pc-windows-msvc
```

### 3.2 ⚠️ 坑 1：Homebrew Rust 没有 Windows target（首次失败）

本机 `which rustc` 首先命中 `/opt/homebrew/bin/rustc`（Homebrew 1.99.0）。首次交叉编译直接失败，**错误原文**：

```text
error[E0463]: can't find crate for `std`
  |
  = note: the `x86_64-pc-windows-msvc` target may not be installed
  = help: consider downloading the target with `rustup target add x86_64-pc-windows-msvc`
error: could not compile `xwin-warm` (bin "xwin-warm") due to 1 previous error
```

原因：Homebrew 的 rustc sysroot 是 `/opt/homebrew/Cellar/rust/1.99.0_1`，其中没有 Windows target；`rustup target add` 只影响 rustup 管理的 toolchain（本机 stable = 1.98.1，已装好该 target）。

**修复方式**（后续所有交叉编译命令都这样执行）：

```bash
export PATH="$HOME/.cargo/bin:$PATH"   # 让 rustup 的 cargo/rustc 优先
export RUSTUP_TOOLCHAIN=stable         # 强制使用 rustup stable toolchain
cargo xwin build --release --target x86_64-pc-windows-msvc -p paranoia-enc-cli
```

### 3.3 ⚠️ 坑 2：首次运行 cargo-xwin 需要联网下载 MSVC SDK/CRT

首次执行时日志：

```text
⏬ Downloading MSVC CRT...
✅ Downloaded MSVC CRT in 59s.
```

- 缓存位置（macOS）：`~/Library/Caches/cargo-xwin`，实测约 **1.1 GB**；
- 之后可离线复用；清理后需重新下载；
- 交叉编译不依赖安装完整 Visual Studio，但 `clang-cl`（brew llvm）必须可用。

## 4. 交叉编译证据：CLI + core → Windows x64

### 4.1 冒烟：任意 crate 能否产出 PE32+

先用一个最小 crate 验证工具链（`/tmp/xwin-warm`）：

```bash
cd /tmp/xwin-warm   # cargo init 的 hello world
export PATH="$HOME/.cargo/bin:$PATH" && export RUSTUP_TOOLCHAIN=stable
cargo xwin build --release --target x86_64-pc-windows-msvc
```

结果：

```text
Finished `release` profile [optimized] target(s) in 0.75s
target/x86_64-pc-windows-msvc/release/xwin-warm.exe: PE32+ executable (console) x86-64, for MS Windows
（大小 130,560 B）
```

### 4.2 正式产物：paranoia-enc-cli.exe

```bash
cd /Users/nanase/DSH/paranoia-enc-decryptor
export PATH="$HOME/.cargo/bin:$PATH" && export RUSTUP_TOOLCHAIN=stable
cargo xwin build --release -p paranoia-enc-cli --target x86_64-pc-windows-msvc
```

结果（完整依赖树交叉编译成功，包含 C 依赖）：

```text
   Compiling zstd-sys v2.1.2+zstd.1.5.7      # C 代码（zstd），用 clang-cl 交叉编译
   Compiling blake3 v1.8.7                  # 含 C/汇编实现
   Compiling paranoia-enc-core v0.1.0
   Compiling paranoia-enc-cli v0.1.0
    Finished `release` profile [optimized] target(s) in 12.36s
```

产物：

```text
-rwxr-xr-x  781312  target/x86_64-pc-windows-msvc/release/paranoia-enc-cli.exe
file: PE32+ executable (console) x86-64, for MS Windows
```

导入表（`llvm-objdump -p` 实测，说明运行时依赖）：

```text
KERNEL32.dll      # Windows 基础 API
ntdll.dll         # NtReadFile / NtWriteFile 等
bcryptprimitives.dll  # ProcessPrng（随机数）
api-ms-win-core-synch-l1-2-0.dll  # WaitOnAddress / WakeByAddress*
VCRUNTIME140.dll  # C++ 运行时（MSVC 构建的 Rust 程序常规依赖）
```

> 注意：`VCRUNTIME140.dll` 需要目标机器安装 **Microsoft Visual C++ Redistributable**（绝大多数 Windows 机器已有；CI 的 Windows runner 自带）。这是 MSVC toolchain 的常规依赖，不代表产物异常。

### 4.3 这个证据能说明/不能说明什么

| 能说明 | 不能说明 |
|---|---|
| core + CLI 的 Rust 代码与全部第三方依赖（含 C/汇编依赖）可编译为 Windows x64 PE | exe 在 Windows 上能启动 |
| 交叉编译工具链配置正确、步骤可复现 | 解密结果在 Windows 上与 macOS 一致（未运行） |
| 无 Windows 专属编译错误（`cfg(windows)` 分支可编译） | GUI 可构建/可运行 |

## 5. Tauri GUI 的 Windows 交叉编译调查

> 结论先行：Tauri v2 GUI 的 **Rust 侧可以交叉编译**（本机实测成功，产出裸 exe，见 §5.2）；但 **Windows 安装包不能**从 macOS 交叉打包（bundle 依赖 Windows 原生工具，见 §5.3），且产物**从未在 Windows 上运行过**。真实可交付的 GUI 构建路径仍然是 CI（§6）+ 真机验收（§7）。

### 5.1 尝试的命令

```bash
cd gui/src-tauri
export PATH="$HOME/.cargo/bin:$PATH" && export RUSTUP_TOOLCHAIN=stable
cargo xwin build --release --target x86_64-pc-windows-msvc
```

### 5.2 实测结果

实测：**成功（EXIT=0）**。

```text
cd gui/src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
# ...（编译完整 Tauri / wry / webview2-com 依赖树，约 5 分钟）
    Finished `release` profile [optimized] target(s) in 4m 58s
EXIT=0
```

产物：

```text
-rwxr-xr-x  6,886,912  target/x86_64-pc-windows-msvc/release/paranoia-enc-decryptor.exe
file: PE32+ executable (GUI) x86-64, for MS Windows
PE Subsystem: 00000002 (Windows GUI)；含 .rsrc 资源节（0x5698，图标/清单已嵌入）
```

导入 DLL（`llvm-objdump -p` 实测，节选）：

```text
kernel32.dll / user32.dll / gdi32.dll / shell32.dll / shlwapi.dll
ole32.dll / oleaut32.dll / combase.dll（COM）
comctl32.dll / dwmapi.dll / imm32.dll / advapi32.dll / ntdll.dll
bcryptprimitives.dll（随机数）
api-ms-win-crt-*（UCRT）/ api-ms-win-core-synch-l1-2-0.dll / api-ms-win-core-winrt-error-l1-1-0.dll
```

> 注意：导入表里**没有** `WebView2Loader.dll` —— Tauri 2（webview2-com）把 loader 静态链接进 exe；但**运行时仍要求目标系统安装 WebView2 Runtime**（Windows 10/11 通常预装）。

构建过程中的告警（**非错误**，可忽略）：链接阶段出现大量 LNK4099，例如：

```text
lld-link: Cannot use debug info for 'libcmt.lib(env_mode.obj)' [LNK4099]
>>> failed to load reference 'D:\a\_work\1\s\binaries\amd64ret\lib\amd64\libcmt.amd64.pdb': No such file or directory
...
warning: `paranoia-enc-decryptor` (bin "paranoia-enc-decryptor") generated 1 warning
```

原因：cargo-xwin 下载的 MSVC 运行库不带 PDB 调试符号，链接器只是无法合并调试信息，**不影响链接与产物**；最终 EXIT=0。

**诚实边界**：本节只有「编译链接成功」这一条证据——没有生成安装包，该 exe 也**从未在 Windows 上启动过**（本机无 Windows/VM/Wine）。

### 5.3 Tauri 打包（bundle）层面的限制

上文 §5.2 已实测「GUI 的 Rust 部分能交叉编译」，但 `pnpm tauri build --bundles nsis` 仍无法在 macOS 上产出 Windows 安装包：Tauri 的打包器需要目标平台原生工具（NSIS `makensis` / WiX / Windows 资源编辑），官方不支持从 macOS 交叉打包 Windows 安装器。因此：

- Windows 安装包**只能**在 Windows 上构建（CI `windows-latest` 或真机）；
- 本机 `cargo xwin` 只用于回答「GUI 的 Rust 依赖树能否在 Windows target 下编译」这一构建层面的问题。

## 6. CI：Windows 从零构建的权威路径

`.github/workflows/build.yml`（与 macOS 同一份 workflow，矩阵构建）：

- runner：`windows-latest`（Windows Server x64，GitHub 托管）；
- 工具链：pnpm 11 → Node.js 22 → `dtolnay/rust-toolchain@stable`（带 `x86_64-pc-windows-msvc` target）→ `Swatinem/rust-cache`；
- 步骤：`cargo test --workspace` → CLI 冒烟（`cargo run --release -p paranoia-enc-cli -- --help`）→ `cd gui && pnpm install --frozen-lockfile` → `pnpm tauri build --bundles nsis` → 上传产物；
- 产物：`paranoia-decryptor-windows-x64` artifact，包含 `gui/src-tauri/target/release/bundle/nsis/*-setup.exe` 与裸 exe；
- 触发：push 到 main / pull_request / 手动 `workflow_dispatch`（Actions 页面可下载 artifact）。

> 状态说明：本机无远程 Git 仓库（`.git/config` 未配置 remote），因此该 workflow **尚未在 GitHub Actions 上真实跑过**；其 YAML 已通过本地 YAML 解析校验（Ruby Psych）。它是 Windows 构建的设计路径，真实运行结果以首次 CI 执行日志为准。

## 7. 如果你有一台 Windows x64 机器：验收清单

```powershell
# 1. 安装依赖：Rust(rustup, MSVC toolchain) + Node.js >= 20 + pnpm + VS Build Tools(C++ 桌面开发)
# 2. 构建 + 单元测试
git clone <repo-url>; cd paranoia-enc-decryptor
cargo test --workspace
./scripts/build-windows.ps1          # 或: cd gui; pnpm install; pnpm tauri build --bundles nsis

# 3. CLI 端到端（对照 README 的 SHA-256 表）
copy <sample>\3138.7z.002.enc %TEMP%\sample.enc
cargo run --release -p paranoia-enc-cli -- -p 123 -o %TEMP%\out %TEMP%\sample.enc
certutil -hashfile %TEMP%\out\3138.7z.002 SHA256
# 预期: 46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840

# 4. GUI 手工验收
#    - 双击 bundle\nsis\*-setup.exe 安装（SmartScreen 需「更多信息 -> 仍要运行」）
#    - 打开「Paranoia 解密工具」：拖放 .enc、多选添加、输入密码 123、默认输出到首文件目录
#    - 观察逐文件/整体进度、错误提示、冲突策略（rename/overwrite/error）
#    - 勾选「记住密码」后重启，确认自动填充；点「清除记忆」后重启确认不再填充
#    - 校验输出 SHA-256 与 README 对照表一致
```

Windows 特有检查点：

- WebView2 Runtime 是否存在（Windows 10/11 通常预装；GUI 窗口白屏时优先排查）；
- 安装目录/输出目录的权限与中文路径；
- 密码存储路径：`%APPDATA%\paranoia-enc-decryptor\config.json`（明文，见 README 风险说明）。

## 8. 不可声称的结论（诚实边界）

本文档**不**声称以下内容（因为没有证据）：

- ❌ 「已在 Windows x64 上运行通过」；
- ❌ 「Windows 安装包已实测可安装」；
- ❌ 「Windows GUI 功能已验收」；
- ❌ 「CI workflow 已成功运行过」——本机无 remote，未触发 GitHub Actions。

有证据的结论只有：**core + CLI 的 Windows x64 交叉编译成功，产物为合法 PE32+；GUI 的 Windows 构建路径为 CI（windows-latest）+ 真机验收清单（§7）。**
