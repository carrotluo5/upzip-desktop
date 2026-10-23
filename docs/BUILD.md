# 构建说明（BUILD）

> 本文档面向**从源码构建**的开发者，补充 [README.md](../README.md) 中的「构建与运行」一节。
> Windows x64 的交叉编译调查与真机验证限制见 [WINDOWS_BUILD.md](WINDOWS_BUILD.md)。

## 1. 前置依赖

| 依赖 | 最低要求 | 本项目开发/验证版本 | 安装方式 |
|---|---|---|---|
| Rust stable | 1.80+ | rustc 1.99.0（Homebrew）/ 1.98.1（rustup，交叉编译用） | <https://rustup.rs> 或 `brew install rust` |
| Node.js | ≥ 20 | v26.10.0 | `brew install node` / nvm / 官网安装包 |
| pnpm | ≥ 9 | 11.27.0 | `corepack enable` 或 `npm i -g pnpm` |
| Xcode Command Line Tools | — | Apple clang 21（`/Library/Developer/CommandLineTools`） | `xcode-select --install` |
| Visual Studio Build Tools | 含 C++ 桌面开发 | **未在本机验证**（macOS 机器） | <https://visualstudio.microsoft.com/visual-cpp-build-tools/> |
| WebView2 Runtime | — | Windows 10/11 通常预装 | <https://developer.microsoft.com/microsoft-edge/webview2/> |
| Tauri CLI | v2 | 2.12.4（`@tauri-apps/cli`，由 pnpm 管理） | `cd gui && pnpm install` |

> macOS 上若同时装了 Homebrew Rust 和 rustup Rust，`which cargo` 可能指向 `/opt/homebrew/bin/cargo`。**常规构建不受影响**；仅 Windows 交叉编译需要 rustup 的 toolchain（见 [WINDOWS_BUILD.md](WINDOWS_BUILD.md)）。

## 2. 快速开始

### macOS arm64

```bash
git clone <repo-url> paranoia-enc-decryptor
cd paranoia-enc-decryptor
./scripts/build.sh
```

脚本会依次执行：环境检查 → `cargo test --workspace` → `cd gui && pnpm install` → `pnpm tauri build` → 打印产物路径与大小。

> **本机实测**（macOS 27.2 arm64，2026-10-24，v0.1.1）：`./scripts/build.sh` 全流程通过；Rust 测试 35 个 core 单元测试 + 1 个 oracle 向量测试通过（2 个真实样本测试默认 ignored）；GUI release 增量构建 29.80 s；产物 `.app` 5.8 MiB、`.dmg` 4.25 MiB（4,457,433 B）、裸二进制 4,888,448 B。

### Windows x64

```powershell
git clone <repo-url> paranoia-enc-decryptor
cd paranoia-enc-decryptor
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

在 macOS 上交叉编译 Windows 单文件便携版（实验性，需 `cargo-xwin`，见 §8）：

```bash
./scripts/build-windows-portable.sh
```

### 手动等价命令

```bash
# 1. Rust 侧（core + CLI）
cargo test --workspace
cargo build --release --workspace

# 2. GUI（独立 Cargo workspace，位于 gui/src-tauri）
cd gui
pnpm install
pnpm tauri build              # macOS: .app + .dmg；Windows: NSIS（建议显式 --bundles nsis）
```

## 3. 构建产物

| 平台 | 产物 | 路径（相对仓库根） |
|---|---|---|
| macOS arm64 | 应用包 | `gui/src-tauri/target/release/bundle/macos/Paranoia Decryptor.app`（v0.1.1 本机实测 5.8 MiB） |
| macOS arm64 | DMG 安装镜像 | `gui/src-tauri/target/release/bundle/dmg/Paranoia Decryptor_<version>_aarch64.dmg`（v0.1.1 本机实测 4,457,433 B / 4.25 MiB） |
| macOS arm64 | 裸二进制 | `gui/src-tauri/target/release/paranoia-enc-decryptor`（v0.1.1 本机实测 4,888,448 B） |
| Windows x64 | NSIS 安装包 | `gui\src-tauri\target\release\bundle\nsis\*-setup.exe` |
| Windows x64 | 裸二进制 / 单文件便携版 | `gui\src-tauri\target\release\paranoia-enc-decryptor.exe`（真机构建）；交叉编译便携版在 `gui/src-tauri/target/x86_64-pc-windows-msvc/release/paranoia-enc-decryptor.exe`（v0.1.1 实测 6,980,096 B） |
| 通用 | CLI（验证/脚本用） | `target/release/paranoia-enc-cli`（Windows: `.exe`） |

Tauri 的应用名、标识符与打包目标定义在 `gui/src-tauri/tauri.conf.json`：

- `productName = "Paranoia Decryptor"`
- `identifier = "com.nanase.paranoia-decryptor"`
- `bundle.targets = "all"`（macOS 实际产出 `.app` + `.dmg`；Windows 可用 `pnpm tauri build --bundles nsis` 只产 NSIS）

## 4. 开发模式

```bash
cd gui && pnpm install
pnpm tauri dev      # 开发窗口（热加载前端；等价于 pnpm dev）
```

前端为纯静态资源（`gui/src/`，`build.frontendDist = "../src"`），无打包步骤；修改 `gui/src/*` 后由 Tauri dev server 直接刷新。Rust 侧（core / src-tauri）修改后需重新编译。

## 5. 目录与构建缓存

| 路径 | 说明 |
|---|---|
| `target/` | 根 workspace（`paranoia-enc-core` + `paranoia-enc-cli`）的 Cargo 构建缓存 |
| `gui/src-tauri/target/` | Tauri GUI 的独立 Cargo 构建缓存（`gui/src-tauri` 自带空 `[workspace]` 表，不并入根 workspace） |
| `gui/node_modules/` | pnpm 依赖（`@tauri-apps/cli` 等） |
| `gui/pnpm-lock.yaml` | pnpm 锁文件，CI 使用 `--frozen-lockfile` 安装 |

清理：`cargo clean`、`rm -rf gui/src-tauri/target`、`rm -rf gui/node_modules`（按需，全部重建耗时较长）。

## 6. 故障排查

| 症状 | 原因 / 解决 |
|---|---|
| `command not found: pnpm` | `corepack enable` 或 `npm i -g pnpm`；确认 Node ≥ 20 |
| `xcrun: error: invalid active developer path` | 执行 `xcode-select --install`，或 `sudo xcode-select -s /Library/Developer/CommandLineTools` |
| `pnpm install --frozen-lockfile` 报 lockfile 不一致 | 本地先 `pnpm install` 更新 `gui/pnpm-lock.yaml` 并提交 |
| macOS 提示「无法打开，因为无法验证开发者」 | 未签名构建属预期：右键 App →「打开」，或系统设置 → 隐私与安全性 →「仍要打开」 |
| Windows SmartScreen 拦截 | 未签名安装包属预期：更多信息 → 仍要运行 |
| Windows 构建缺少 `link.exe` / MSVC | 安装 Visual Studio Build Tools（C++ 桌面开发工作负载）后重开终端 |
| WebView2 缺失（Windows 旧系统） | 安装 WebView2 Evergreen Runtime |
| Tauri 构建很慢/占空间 | 首次需编译整个 Tauri/Rust 依赖树；`gui/src-tauri/target` 可达数 GB，属正常 |
| 交叉编译报 `can't find crate for std` | 用的是 Homebrew Rust（sysroot 无 Windows target）。改用 rustup 的 `stable` toolchain（详见 WINDOWS_BUILD.md） |

## 7. CI（GitHub Actions）

`.github/workflows/build.yml` 在以下场景运行：`push`（main）、`pull_request`、`workflow_dispatch`。

- 矩阵：`macos-14`（arm64）与 `windows-latest`（x64）；
- 步骤：checkout → pnpm → Node 22 → Rust stable（带目标 target）→ 缓存 → `cargo test --workspace` → CLI 冒烟 → `pnpm install` → `pnpm tauri build`（Windows 用 `--bundles nsis`）→ 上传产物；
- 产物名：`paranoia-decryptor-macos-arm64`（`.app` + `.dmg`）、`paranoia-decryptor-windows-x64`（`*-setup.exe` + exe）。

CI 是 Windows 侧**真实构建验证**的权威路径；本仓库开发机为 macOS arm64，无法在本地做 Windows 运行时验证。

> 注意：本仓库当前**没有配置 git remote**，因此该 workflow 尚未在 GitHub 上实际运行过；一旦推送到 GitHub 即按上述矩阵执行。

## 8. Windows 交叉编译（可选，实验性）

在 macOS 上可用 `cargo-xwin` 交叉编译到 `x86_64-pc-windows-msvc`：CLI/core 与 Tauri GUI **单文件便携 exe** 均已实测成功，但**不能生成 NSIS 安装包**、也不能在 macOS 上验证 Windows 运行时。完整命令、版本、静态 CRT 失败记录与产物检查见 **[WINDOWS_BUILD.md](WINDOWS_BUILD.md)**。

```bash
# 前置：brew install llvm && cargo install cargo-xwin
#       rustup target add --toolchain stable x86_64-pc-windows-msvc
export PATH="$HOME/.cargo/bin:$PATH"   # 确保用 rustup 的 cargo/rustc
RUSTUP_TOOLCHAIN=stable cargo xwin build --release -p paranoia-enc-cli --target x86_64-pc-windows-msvc

# GUI 单文件便携版（等价于脚本 build-windows-portable.sh）
cd gui/src-tauri
RUSTUP_TOOLCHAIN=stable cargo xwin build --release --target x86_64-pc-windows-msvc
```

## 9. 版本记录

| 组件 | 版本 |
|---|---|
| 项目 | 0.1.1（Phase 1） |
| Rust edition | 2021 |
| Tauri | v2（CLI 2.12.4） |
| 验证环境 | macOS arm64（Apple Silicon），rustc 1.99.0 / 1.98.1，node 26.10.0，pnpm 11.27.0 |
