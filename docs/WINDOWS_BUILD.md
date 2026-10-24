# Windows 构建（WINDOWS_BUILD）

本项目支持两条 Windows 路线。**macOS 上只能出"便携版 zip"；NSIS 安装包必须在
真实 Windows 上构建。** 两条路线都没有在真实 Windows 上运行验证过——这是本项目
当前最大的验证缺口，详见 §5「诚实边界」。

## 1. 路线 A：macOS 交叉编译便携版（本仓库实测可跑通编译）

```bash
# 一次性准备
brew install llvm
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc
export PATH="$HOME/.cargo/bin:$PATH"     # Homebrew 的 rust 默认不带 Windows target

# 构建
./scripts/build-windows-portable.sh
```

脚本流程：

1. 检查 `cargo-xwin`、`x86_64-pc-windows-msvc` target、`binaries/win/7z.exe|7z.dll`；
2. `pnpm --dir gui install --frozen-lockfile` + `pnpm --dir gui build`（前端 dist）；
3. `touch gui/src-tauri/build.rs`（避免图标/资源缓存导致 tauri-build 不重跑）；
4. `cargo xwin build --release --target x86_64-pc-windows-msvc -p unparanoia -p unparanoia-cli`；
5. 组装 `target/windows-portable/Unparanoia-<ver>-win-x64-portable/`：
   `Unparanoia.exe`（GUI）、`unparanoia-cli.exe`、`7z.exe`、`7z.dll`、
   `7z-LICENSE.txt`、`README.txt`、`SHA256SUMS`；
6. 用随附 7zz 打成 `...-portable.zip`，打印 zip 与两个 exe 的大小/SHA-256。

便携包运行要求：Windows 10/11 **x64**；Windows 11 自带 WebView2 Runtime，
Windows 10 需装 Evergreen Runtime；免安装、无需管理员权限、不写系统目录。
GUI 会在 exe 同目录找 `7zz.exe/7z.exe`（以及 `binaries/win/7z.exe`），
所以必须解压整包使用，不能只拷 exe。

首次 `cargo xwin build` 会下载约 1.1 GB 的 MSVC CRT/SDK 到
`~/Library/Caches/cargo-xwin`；`LNK4099: PDB 'libcmt.pdb' was not found` 之类
警告可忽略。**不要**给 xwin 加 `-C target-feature=+crt-static`：
实测会因 webview2-com 的 `_seh_filter_dll` 未定义而链接失败，保持动态 CRT。

## 2. 路线 B：真实 Windows 上构建 NSIS 安装包

前置：Rust stable（MSVC toolchain）+ Visual Studio C++ Build Tools、Node.js >= 20、
pnpm 11、WebView2 Runtime。

```powershell
.\scripts\build-windows.ps1              # cargo test --workspace + tauri build --bundles nsis
.\scripts\build-windows.ps1 -SkipTests   # 跳过测试
.\scripts\build-windows.ps1 -Portable    # 只出 exe（不打包），等价便携版
```

产物：

| 产物 | 路径 |
|---|---|
| NSIS 安装包 | `target\release\bundle\nsis\Unparanoia_<ver>_x64-setup.exe` |
| GUI exe | `target\release\unparanoia.exe` |
| CLI exe | `target\release\unparanoia-cli.exe` |

Windows 专用 Tauri 配置在 `gui/src-tauri/tauri.windows.conf.json`：
资源带 `binaries/win/7z.exe`、`7z.dll`、许可证；NSIS 安装界面语言为简体中文 + English。
安装包会把 7-Zip 放进安装目录的 `binaries/win/`，应用运行时从资源目录解析
（见 `crates/unpack-core/src/sevenzip.rs` 的 `SevenZip::locate` 查找顺序）。

## 3. 从代码到产物的路径（避免旧文档的坑）

- 单 Cargo workspace：`gui/src-tauri` 是成员，**产物在仓库根 `target/`**，
  旧 v0.1.1 文档里的 `gui/src-tauri/target` 已不存在。
- `pnpm --dir gui tauri build` 的 `beforeBuildCommand` = `pnpm sync:binaries && pnpm build`，
  会先把 `binaries/` 同步到 `gui/src-tauri/binaries/`（gitignored 生成物）。
- 改了图标、`tauri.conf.json` 或 7-Zip 二进制后，`touch gui/src-tauri/build.rs`
  可强制重跑 tauri-build；否则 Windows 资源段可能是旧的。

## 4. 已知坑

| 现象 | 原因 / 处理 |
|---|---|
| `error: linking with link.exe failed` / 找不到 MSVC | 没装 VS Build Tools，或没用 `cargo xwin` |
| `undefined symbol: _seh_filter_dll` | 静态 CRT（`+crt-static`）与 webview2-com 冲突；去掉该 RUSTFLAGS |
| `cargo xwin` 下载卡住 | 首次拉 MSVC SDK 约 1.1 GB，检查网络/代理 |
| Homebrew rust 找不到 windows target | `rustup target add x86_64-pc-windows-msvc`，并确保 PATH 用 rustup 的 cargo |
| NSIS 打包失败（在 macOS 上） | 不支持交叉打 NSIS；用便携版 zip，或去真实 Windows 构建 |
| exe 启动报缺少 WebView2 | Win10 装 Evergreen Runtime；Win11 默认自带 |

## 5. 诚实边界（必须保留）

**已验证**（macOS 主机交叉编译，2026-10-24 16:16，`./scripts/build-windows-portable.sh` exit 0，
仅有 LNK4099 之类 PDB 警告）：

| 产物 | 大小 (B) | SHA-256 |
|---|---|---|
| 便携 zip `target/windows-portable/Unparanoia-0.3.0-win-x64-portable.zip` | 5,855,710 | `8799f0476b5d5247f089071fcbd5a1d75c51f2d8260149c3738e8c06b99df187` |
| 包内 GUI `Unparanoia.exe` | 10,962,944 | `15df510b5f8ec458748fe2cdb9ac97e7affa516ee69f7dc0c33e2c35918f8086` |
| 包内 CLI `unparanoia-cli.exe` | 3,584,000 | `34ceb172e448a00f2902046cc57f49a5dc140f43806e3267a335b4c224de0a50` |
| 包内 `7z.exe` / `7z.dll` / `7z-LICENSE.txt` | 575,488 / 1,906,176 / 6,029 | 与 `binaries/win/` 一致（`4cd7d776…` / `5bd20fb3…` / `99df5a51…`） |

- 两个 exe 均为 PE32+ x86-64（GUI / console）；zip 用 `7zz l` 核对含 7 个文件、
  1 个目录，zip 内 `SHA256SUMS` 与各文件自洽；
- 7-Zip 二进制为官方 25.01 x64（PE 版本资源 25.01 / 2025-08-03 / Igor Pavlov，
  无 Authenticode 签名），哈希见 binaries/README.md；
- 重建会改变 exe（构建时间）与 zip（内层时间戳）的哈希，以当次记录为准。

**未验证 / 已知环境问题**：

- macOS 侧 DMG 打包在 2026-10-24 曾失败（tauri 只报 `failed to run bundle_dmg.sh`）：
  根因是 `target/release/bundle/macos/` 残留了被中断构建留下的半成品同名 dmg 与
  `rw.*.dmg`，`hdiutil convert` 遇目标已存在而失败；清理后重跑成功（v0.3.0 最终
  构建即如此产出，见 BUILD.md §7 与 VERIFICATION.md §7）。
  与 Windows 交叉编译无关，Windows 便携包产物不受影响。
- 未在真实 Windows 上启动 GUI、跑 CLI、加载 WebView2；
- 未验证 Windows 路径/盘符/中文路径、长路径、UAC、只读目录行为；
- 未验证 7z.exe 子进程在 Windows 的密码传递（本项目通过 argv/环境变量传密码，
  同机其他进程理论上可读——Windows 上无有效隔离手段，属于已知风险）；
- 未验证 NSIS 安装/卸载、开始菜单快捷方式、开机自启；
- 未做 Authenticode 签名，SmartScreen 会提示"未知发布者"。

首次正式发布前，必须在一台真实 Windows 11 x64 上执行：
`unparanoia-cli.exe doctor`、真实样本解密、GUI 拖放、安装/卸载各一遍，
并把结果补进 VERIFICATION.md。
