# Upzip

**拖入即解的桌面批量解压工具** —— 自动识别压缩格式、分卷、伪装容器与 `.enc` 加密文件，并按密码库把多层嵌套压缩包一次解到底。

支持 **macOS 12+（Apple Silicon）** 与 **Windows 10/11（x64）**。Rust 解包内核 + Tauri 桌面端，提供图形界面与命令行（`upzip-cli`）。

![Upzip 界面](docs/screenshots/gui-0.4.0.png)

## 核心功能

- **常见格式全覆盖**：支持 `zip` / `7z` / `rar` / `tar` / `gz` / `xz` / `bz2` / `cab` / `iso` 等 7-Zip 支持的格式。
- **自动拼合分卷**：自动识别 `.7z.001`、`.zip.001`、`.part1.rar` 与 `.enc` 分卷并归为同一任务。
- **伪装包与自解压**：能从图片/音视频等伪装头后面剥离出真实压缩包；对自解压 EXE 只安全解包，绝不执行。
- **Paranoia v4（`.enc`）解密**：支持解密 Paranoia File Encryption v4（SSEFE）容器；解出的内层压缩包会继续解。
- **多层嵌套与智能试密码**：默认最多自动拆解 4 层嵌套，每层可独立匹配密码（依次尝试 `-p` / 环境变量 / 密码库 / 注释提示）。
- **文档容器保护**：默认不再继续拆开 `.docx`、`.xlsx`、`.jar`、`.apk` 等以 ZIP 封装的成品文件，避免把文档解成一堆 XML（需要时可通过 `--force-documents` 强制拆解）。

> **不做什么**：不提供加密/打包功能；不支持旧版 Paranoia v3 与 `PFTEREC1` 恢复容器；不联网，也不做自动更新。

## 下载与安装

前往 [Releases](https://github.com/carrotluo5/upzip-desktop/releases) 下载对应平台产物（`v0.4.1`）：

| 平台 | 文件 | 说明 |
|---|---|---|
| macOS 12+（Apple Silicon） | `Upzip-0.4.1-macos-arm64.dmg` | 桌面安装包（拖入「应用程序」即可） |
| macOS 12+（Apple Silicon） | `Upzip-0.4.1-macos-arm64.app.zip` | 免镜像版 `.app` |
| Windows 10/11（x64） | `Upzip-0.4.1-win-x64-portable.zip` | 免安装便携包，含 `Upzip.exe` 与 `upzip-cli.exe` |
| 校验文件 | `SHA256SUMS.txt` | 所有产物的 SHA-256 校验和 |

**首次打开提示**：
- **macOS**：当前包仅做本地 ad-hoc 签名、未公证，首次打开若提示无法验证，请**右键应用 → 选择「打开」**（或在系统设置「隐私与安全性」中点击「仍要打开」）。
- **Windows**：未签 Authenticode，首次运行若弹出 SmartScreen 提示请选择「仍要运行」；Windows 10 若缺少运行环境需安装 Microsoft WebView2 Runtime（Windows 11 通常自带）。首次启动会在 `%LOCALAPPDATA%\Upzip` 自动释放并缓存内嵌的 7-Zip 25.01。

## 快速上手

### 桌面端 GUI

1. 将一个或多个压缩文件/文件夹直接**拖入窗口**（支持同时混入普通包、分卷与 `.enc`）。
2. 输入密码（可勾选存入左侧**「密码库」**），或直接点击开始，由程序自动用已存密码试解。
3. 支持随时暂停、取消或重试失败项；右下角/设置页可切换「跳过已处理」、复制**诊断信息**或打开日志目录排查问题。

### 命令行（`upzip-cli`）

Windows 便携包内已自带 `upzip-cli.exe`；macOS 需从源码编译（`cargo build --release -p upzip-cli`）。

```bash
# 环境自检 / 识别格式 / 查看 .enc 头信息
upzip-cli doctor
upzip-cli detect 文件.enc
upzip-cli inspect 文件.enc

# 解压单个或批量处理（支持分卷自动归并）
upzip-cli run 文件.enc -p 密码 -o 输出目录
upzip-cli run 待解压目录 -p 密码1 -p 密码2 -j 4 --json
upzip-cli run 3138.7z.001.enc 3138.7z.002.enc

# 管理密码库 / 清理异常中断残留的 .upzip-tmp-* 临时目录
upzip-cli config --add-password 密码
upzip-cli cleanup-staging --root ~/Downloads
```

<details>
<summary>常用参数、密码尝试顺序与退出码</summary>

- **常用 `run` 参数**：`-p/--password`（可重复传入）、`-o/--output-dir`、`-j/--jobs`、`--conflict rename|skip|overwrite`、`--layout smart|folder|direct`、`--skip-processed`（跳过已处理指纹）、`--try-empty-password`、`--force-documents`、`--no-recursive`、`--no-nested`、`--json`。
- **密码匹配顺序**：命令行 `-p` → 环境变量 `UPZIP_PASSWORD`（兼容 `UNPARANOIA_PASSWORD` / `PARANOIA_ENC_PASSWORD`）→ 本地密码库 → 压缩包注释猜测 → 空密码（需 `--try-empty-password` 或 GUI 开关）。
- **退出码**：`0` 全部成功/跳过；`1` 存在失败或缺少密码；`2` 参数/配置/环境错误；`130` Ctrl-C 中断。

</details>

## 配置、数据与安全说明

| 平台 | 默认配置目录 |
|---|---|
| macOS | `~/Library/Application Support/upzip/` |
| Windows | `%APPDATA%\upzip\` |

可通过环境变量 `UPZIP_CONFIG_DIR` 自定义位置。目录内主要包含：
- `config.json`：本地密码库（**明文保存**，Unix 上权限 `0600`）
- `settings.json`：界面与解压偏好设置
- `processed.json`：已成功处理的文件指纹（仅在开启「跳过已处理」/ `--skip-processed` 时跳过）
- `logs/upzip.log`：运行日志（超过 2 MiB 自动轮转为 `.1`）

**安全与局限提醒**：
- **密码明文存储**：密码库仅为方便日常批量试密，并非加密保险箱，**请勿在公共/共享电脑上存放重要密码**。
- **7-Zip 密码传递**：macOS/Linux 默认通过标准输入（`stdin`）向 7-Zip 子进程传密；Windows 默认通过命令行参数（`-p`）传递，同机其他进程理论上可能在进程列表中看到。
- **7-Zip 子进程安全**：软件绝不会执行解出的文件，但处理未知/不可信压缩包时仍存在 7-Zip 解析风险，请谨慎对待来源不明的文件。
- **测试边界**：macOS GUI 与 CLI、Windows CI 构建与测试（`cargo test` + `upzip-cli.exe doctor`）均已验证；Windows 桌面 GUI 的真实图形窗口交互暂未在物理机上人工手测。
- **旧版迁移**：`v0.4.0` 由原名 **Unparanoia** 更名为 **Upzip**，首次启动会自动导入旧配置目录与密码库。

## 从源码构建

需要 Rust >= 1.80、Node.js >= 20、pnpm 11.x（macOS 另需 Xcode Command Line Tools）：

```bash
./scripts/build.sh                    # macOS：运行测试 + 前端构建 + 打包 .app/.dmg + 构建 CLI
./scripts/build-windows-portable.sh   # 在 macOS 上交叉编译 Windows 便携版 zip
cargo build --release -p upzip-cli    # 仅编译命令行工具
pnpm --dir gui tauri dev              # 启动前端 + Tauri 热重载开发模式
```

真 Windows 本机构建脚本见 `scripts/build-windows.ps1`。更多细节与开发文档：
- [docs/BUILD.md](docs/BUILD.md) / [docs/WINDOWS_BUILD.md](docs/WINDOWS_BUILD.md) — 构建与打包说明
- [docs/OUTPUT-LAYOUT.md](docs/OUTPUT-LAYOUT.md) — 输出目录、冲突策略与临时目录清理规则
- [docs/FORMAT.md](docs/FORMAT.md) — Paranoia v4 文件头格式与 KDF 参数
- [docs/CORE_API.md](docs/CORE_API.md) — Rust 核心库接口
- [docs/VERIFICATION.md](docs/VERIFICATION.md) — 测试与验证记录

## 许可证

项目代码采用 **MIT** 协议，详见 [LICENSE](LICENSE)。内嵌及随包分发的 **7-Zip 25.01** 遵循 **GNU LGPL 2.1+**（附带 unRAR 解压限制），详见 [NOTICE](NOTICE) 或运行 `upzip-cli --licenses`。
