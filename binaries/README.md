# binaries/ — 随附的 7-Zip 25.01 二进制

这些文件由 `scripts/sync-binaries.mjs` 同步到 `gui/src-tauri/binaries/`，
再由 Tauri 作为应用资源打包进 Unparanoia（macOS 只带 7zz）。
**v0.3.1 起 Windows 版改为单 exe**：`crates/embedded-7z` 用 `include_bytes!` 把
`win/7z.exe`、`win/7z.dll`、`7z-LICENSE.txt` 直接内嵌进 GUI/CLI exe，运行时解压到
用户缓存目录再以子进程调用；便携包不再附带 sidecar 7z.exe/7z.dll。
**单一真源仍是本目录**；`gui/src-tauri/binaries/` 是生成物（已 gitignore）。

## 文件清单（2026-10-24 记录）

| 文件 | 大小 (B) | SHA-256 | 用途 |
|---|---|---|---|
| `mac/7zz` | 5,885,696 | `5c2fd36f00a66f7787dcf1badd977d44a02b50063fe5678e1f19ff64797432ed` | macOS 通用二进制（x86_64 + arm64），被当作子进程调用 |
| `win/7z.exe` | 575,488 | `4cd7d776c686427226a151789d2d61f0b2ed2c392148cc4e69c0238362fafecf` | Windows x64 控制台版 7-Zip |
| `win/7z.dll` | 1,906,176 | `5bd20fb38499d95c39594f41d4781b6181b3304b7f1f4d06b0182f514e7eaa74` | Windows x64 7z.dll（v0.3.1 起与 7z.exe 一起被内嵌进 Windows exe） |
| `7z-LICENSE.txt` | 6,029 | `99df5a516035baca9f2e8095977599d4a4340f58ba900f870238943996014f53` | 7-Zip 许可证（LGPL + unRAR 限制），随产品分发 |
| `win/7z-LICENSE.txt` | 6,031 | `3a184aa13dc8ad30734e28ff901b478ccbcb5b41be52427a5e7609c8fd9e5ddb` | 许可证副本（与根目录版本仅空白差异；同步脚本只同步根目录版本） |

## 版本与平台自证

```bash
./binaries/mac/7zz i | head -1
# 7-Zip (z) 25.01 (arm64) : Copyright (c) 1999-2025 Igor Pavlov : 2025-08-03
file binaries/mac/7zz
# Mach-O universal binary with 2 architectures: [x86_64] [arm64]
file binaries/win/7z.exe binaries/win/7z.dll
# PE32+ executable (console) x86-64 / PE32+ executable (DLL) (GUI) x86-64
```

Windows PE 版本资源（`llvm-objdump -p` / 资源查看器）：
CompanyName `Igor Pavlov`、ProductName `7-Zip`、FileVersion/ProductVersion `25.01`、
LegalCopyright `Copyright (c) 1999-2025 Igor Pavlov`；PE 链接时间戳
`1754200800` = 2025-08-03（与官方 25.01 发布日一致）。

## 签名状态（不是来源验证）

- `codesign -dv binaries/mac/7zz` → `flags=0x20002(adhoc,linker-signed)`；
  `codesign --verify --strict` 报 "code object is not signed at all"。
- Windows 两个 PE 的 Security Directory 为 0（**无 Authenticode 签名**）。
- 官方 7-Zip 发布也不做这两种签名，因此签名缺失本身**不能**证明二进制被篡改；
  反过来也不能证明来源。可用的来源证据只有上表 SHA-256 + 版本字符串，
  以及"从 upzip 项目原样继承"这一事实（见 NOTICE 的诚实边界）。

## 许可证

7-Zip 主体为 GNU LGPL v2.1+，含 unRAR 限制（允许解压 RAR，禁止逆向 RAR 压缩算法 /
制作 RAR 兼容压缩器）。完整文本见 `7z-LICENSE.txt`；摘要见仓库根目录 `NOTICE`。
官方页面：https://www.7-zip.org/license.txt

## v0.3.1 Windows 单 exe 内嵌方式

- 实现：`crates/embedded-7z`（`SEVEN_ZIP_VERSION = "25.01"`、`LICENSE_TEXT`、
  `ensure_7z() -> Result<Option<PathBuf>, Embedded7zError>`）；Windows 下用
  `include_bytes!` 内嵌 `win/7z.exe`、`win/7z.dll`、`7z-LICENSE.txt`，非 Windows
  返回 `Ok(None)`。
- 首次运行把内嵌运行时解压到 `%LOCALAPPDATA%\Unparanoia\embedded-7z\25.01-r1\`
  （可用 `UNPARANOIA_7Z_CACHE_DIR` 覆盖缓存根目录），之后复用；GUI 找不到时回退
  外部 7-Zip 且不阻止启动，CLI 优先级 `--sevenzip` > 环境变量 > 内嵌 > 自动探测。
- 已核对内嵌字节与本目录文件**逐字节一致**（v0.3.1 交叉编译产物上用 Python
  字节包含检测，`embedded=True`），见 docs/VERIFICATION.md §9。
- 许可证：运行 `unparanoia-cli --licenses` 或 GUI 设置页「7-Zip 许可证」可查看。

## 更新流程

1. 从 https://www.7-zip.org/download.html 下载官方包；
2. 解压出 `7zz` / `7z.exe` / `7z.dll` / `License.txt`，覆盖本目录；
3. `shasum -a 256 binaries/**/*` 更新上表，并同步 NOTICE（含 `crates/embedded-7z`
   的 `include_bytes!` 输入——它直接引用 `binaries/win/` 源文件，无需单独更新）；
4. `node scripts/sync-binaries.mjs`（或 `pnpm --dir gui sync:binaries`）同步镜像；
5. 运行 `./scripts/make-fixtures.sh && ./scripts/e2e-verify.sh` 确认解压链路未回归。
