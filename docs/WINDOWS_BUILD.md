# Windows 构建（WINDOWS_BUILD）

本项目支持两条 Windows 路线。**macOS 上只能出"便携版 zip"；NSIS 安装包必须在
真实 Windows 上构建。** 0.3.1 起便携包是**单 exe**：GUI 与 CLI 都已内嵌 7-Zip 25.01
与前端资源，不再附带 sidecar `7z.exe`/`7z.dll`。0.3.2 起补充 Windows 体验修复：
不自建控制台窗口（`CREATE_NO_WINDOW`）、7-Zip 失败信息含路径/退出码/stderr 尾部、
Windows 锁文件错误（5/32/225）自动重试、日志落盘与设置页诊断信息。两条路线都
**没有在真实 Windows 上运行验证过**——这是本项目当前最大的验证缺口，详见 §5「诚实边界」。

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
4. 分别交叉编译（GUI 必须显式 `--features custom-protocol`，否则 exe 走 devUrl
   `http://localhost:1420`，在用户机器上白屏；该 feature 只属于 GUI crate，不能和
   CLI 混在同一条 `-p` 命令里）：

   ```bash
   cargo xwin build --release --target x86_64-pc-windows-msvc -p unparanoia --features custom-protocol
   cargo xwin build --release --target x86_64-pc-windows-msvc -p unparanoia-cli
   ```

5. 组装单 exe 便携包 `target/windows-portable/Unparanoia-<ver>-win-x64-portable/`：
   `Unparanoia.exe`（GUI，内嵌前端 + 7-Zip）、`unparanoia-cli.exe`（内嵌 7-Zip）、
   `7z-LICENSE.txt`、`README.txt`、`SHA256SUMS`；**不再放** `7z.exe`/`7z.dll`；
6. 用随附 7zz 打成 `...-portable.zip`，打印 zip 与两个 exe 的大小/SHA-256。

便携包运行要求：Windows 10/11 **x64**；Windows 11 自带 WebView2 Runtime，
Windows 10 需装 Evergreen Runtime；免安装、无需管理员权限、不写系统目录。
两个 exe 都已内嵌 7-Zip 25.01：首次运行会把运行时解压到
`%LOCALAPPDATA%\Unparanoia\embedded-7z\25.01-r1\`（可用 `UNPARANOIA_7Z_CACHE_DIR`
覆盖缓存根目录），之后复用。因此**可以只拷 exe 单独运行**，不依赖同目录 sidecar。

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
.\scripts\build-windows.ps1 -Portable    # 构建单 exe 便携包（含 zip + SHA256SUMS）
```

产物：

| 产物 | 路径 |
|---|---|
| NSIS 安装包 | `target\release\bundle\nsis\Unparanoia_<ver>_x64-setup.exe` |
| GUI exe | `target\release\unparanoia.exe` |
| CLI exe | `target\release\unparanoia-cli.exe` |
| 便携 zip（`-Portable`） | `target\windows-portable\Unparanoia-<ver>-win-x64-portable.zip` |

Windows 专用 Tauri 配置在 `gui/src-tauri/tauri.windows.conf.json`（NSIS 安装界面语言
简体中文 + English）。注意：0.3.1 起 GUI/CLI 已内嵌 7-Zip，运行时不再解析 sidecar；
该配置里仍列出的 `binaries/win/7z.exe`、`7z.dll`、许可证资源只是安装包的额外拷贝
（冗余，后续可清理，不影响单 exe 运行）。

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
| exe 启动白屏 / 卡在 `localhost:1420` | 裸 `cargo build` 构建 GUI 时漏了 `--features custom-protocol`；构建脚本与 `tauri build` 已自动带 |
| exe 启动报缺少 WebView2 | Win10 装 Evergreen Runtime；Win11 默认自带 |
| GUI 没有反应/异常，需要日志 | 日志在 `%APPDATA%\unparanoia\logs\unparanoia.log`（>2 MiB 轮转为 `unparanoia.log.1`）；GUI 设置页「诊断信息」可复制诊断 JSON（版本、路径、7-Zip 来源、配置摘要）或「打开日志目录」 |
| 7-Zip 报错看不懂 | v0.3.2 起错误信息包含 7z 路径、OS 错误/退出码、stderr 尾部（≤1000 字符），并提示可设 `UNPARANOIA_7Z` 指向已安装 7-Zip；锁文件错误 5/32/225 会自动重试最多 3 次（间隔 150 ms）后再报错 |
| 第二次拖同一文件没有重新解压 | v0.3.2 起「跳过已处理」**默认关闭**（旧配置一次性迁移）；只有开启后才会按 `processed.json` 跳过。如需重跑：GUI 设置页「清除处理记录」，或 CLI 加 `--skip-processed` 才启用跳过 |
| GUI 解密时闪过黑色控制台窗口 | v0.3.1 及更早的已知问题；v0.3.2 起 7z 子进程使用 `CREATE_NO_WINDOW`，GUI 不再弹窗（真机待验证） |

## 5. 诚实边界（必须保留）

**已验证**（macOS 主机交叉编译，2026-10-24，
`./scripts/build-windows-portable.sh` exit 0，仅有 LNK4099 之类 PDB 警告，与产物无关）。
下表是 **v0.3.2 单 exe 布局的最终记录**；每次重建哈希都会变。

| 产物 | 大小 (B) | SHA-256 |
|---|---|---|
| 便携 zip `target/windows-portable/Unparanoia-0.3.2-win-x64-portable.zip` | 7,160,723 | `465490d7311a1aad4328baf825d0807f9d62d6559e54ff13274c1ae457cc2254` |
| 包内 GUI `Unparanoia.exe`（PE32+ GUI x86-64） | 13,758,464 | `e1c18e51e39aacb186a945ae8efcbbbd7c7008ba9224a32c166feba283fbdf99` |
| 包内 CLI `unparanoia-cli.exe`（PE32+ console x86-64） | 6,211,584 | `ee54556a1c24a79dadedcd25496174e627aa0f0c2fafd953d3332920551c2fa1` |
| 包内 `7z-LICENSE.txt` | 6,029 | 与 `binaries/7z-LICENSE.txt` 一致（`99df5a51…`） |

- zip 用 `7zz l` 核对为 **5 个文件 + 1 个目录**，顶层目录
  `Unparanoia-0.3.2-win-x64-portable/`（两个 exe、`7z-LICENSE.txt`、`README.txt`、
  `SHA256SUMS`；**不再有** `7z.exe`/`7z.dll` 侧车）；解压后
  `shasum -c SHA256SUMS` 全部 OK；
- **单 exe 生效证据**：GUI exe 内 `tauri://localhost` 标记命中（custom-protocol 已
  把前端嵌入，不再是 devUrl `http://localhost:1420`）；
- **内嵌 7-Zip 证据**：两个 exe 均逐字节内嵌 `binaries/win/7z.exe`（575,488 B）、
  `7z.dll`（1,906,176 B）、`7z-LICENSE.txt`（6,029 B），Python 字节包含检测
  `embedded=True`；
- 7-Zip 二进制为官方 25.01 x64（PE 版本资源 25.01 / 2025-08-03 / Igor Pavlov，
  无 Authenticode 签名），哈希见 binaries/README.md；
- `unparanoia-cli --licenses` 可输出内嵌/随包 7-Zip 许可证（LGPL 2.1 + unRAR 限制）；
- v0.3.2 新增 Windows 修复（静态/单测证据，真机未跑）：7z 子进程
  `CREATE_NO_WINDOW`（不弹控制台）、失败信息含路径/退出码/stderr 尾部、
  spawn 锁文件错误 5/32/225 重试 3 次；日志/诊断见 §4 与 VERIFICATION.md §9。
  GUI exe 前端资源经 brotli 压缩，中文串不可直接 grep；单 exe 证据以
  `tauri://localhost` 标记与内嵌 7-Zip 字节包含为准。

> v0.3.0 的 sidecar 布局（zip 内附 7z.exe/7z.dll）已过时，本文件不再保留旧哈希与
> 旧大小，避免误引用。

**未验证 / 已知环境问题**：

- **未在真实 Windows 11 上启动过 GUI/CLI**（用户将真机验证），未验证 WebView2、
  中文路径/盘符、长路径、UAC、只读目录、首次解压缓存
  `%LOCALAPPDATA%\Unparanoia\embedded-7z\25.01-r1\` 的写入权限；
- 未验证 7z.exe 子进程在 Windows 的密码传递（本项目通过 argv/环境变量传密码，
  同机其他进程理论上可读——Windows 上无有效隔离手段，属于已知风险）；
- NSIS 安装包（`scripts/build-windows.ps1` 非 `-Portable` 路线）未构建、未验证；
  正式交付走单 exe 便携版；
- 未做 Authenticode 签名，SmartScreen 会提示"未知发布者"；
- macOS 侧 DMG 打包在 2026-10-24 曾失败（tauri 只报 `failed to run bundle_dmg.sh`）：
  根因是 `target/release/bundle/macos/` 残留了被中断构建留下的半成品同名 dmg 与
  `rw.*.dmg`，`hdiutil convert` 遇目标已存在而失败；清理后重跑成功（v0.3.0/v0.3.1/
  v0.3.2 最终构建均如此，见 BUILD.md §7 与 VERIFICATION.md §7）。与 Windows 交叉编译无关。

首次正式发布前，必须在一台真实 Windows 11 x64 上执行：
`unparanoia-cli.exe doctor`、真实样本解密（含分卷 `.enc` 组）、GUI 拖放、
单 exe 首次运行（确认内嵌 7-Zip 缓存解压成功）、确认无控制台闪窗、
GUI 设置页「诊断信息」与日志文件 `%APPDATA%\unparanoia\logs\unparanoia.log`，
并把结果补进 VERIFICATION.md。

