#!/usr/bin/env bash
#
# unparanoia — 从 macOS 交叉构建 Windows x64 "便携版" zip
#
# 产出: target/windows-portable/Upzip-<version>-win-x64-portable.zip
#   单文件运行：包内只有 Upzip.exe（GUI）、upzip-cli.exe、7z-LICENSE.txt、
#   README.txt、SHA256SUMS。两个 exe 都已内嵌 7-Zip 25.01（7z.exe+7z.dll）与前端资源，
#   不再附带 sidecar 7z.exe/7z.dll。
#
# 前置（一次性）:
#   brew install llvm
#   cargo install cargo-xwin
#   rustup target add x86_64-pc-windows-msvc
#   export PATH="$HOME/.cargo/bin:$PATH"     # Homebrew rust 默认不带 Windows target
#
# 说明:
#   - 这是"无安装器、单 exe"的便携版：解压即用，Windows 11 自带 WebView2 Runtime。
#   - 首次运行会把内嵌的 7-Zip 解压到
#     %LOCALAPPDATA%\Upzip\embedded-7z\25.01-r1（可用 UPZIP_7Z_CACHE_DIR
#     覆盖缓存根目录），之后复用；不需要外部 7-Zip、不需要安装。
#   - 构建 GUI 必须显式 --features custom-protocol：否则 exe 使用 devUrl
#     （http://localhost:1420），在用户机器上会白屏。
#   - 不构建 NSIS 安装包（需要真实 Windows 工具链，见 docs/WINDOWS_BUILD.md）。
#   - 本机只能验证"交叉编译产物存在 + PE 结构正确"；**未在真实 Windows 上运行过**，
#     见 docs/WINDOWS_BUILD.md 的诚实边界。
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

TARGET="x86_64-pc-windows-msvc"
OUT_DIR="${ROOT_DIR}/target/windows-portable"
VERSION="$(grep -E '^version\s*=' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
[[ -n "${VERSION}" ]] || { echo "无法从 Cargo.toml 读取 workspace version" >&2; exit 2; }
PKG_NAME="Upzip-${VERSION}-win-x64-portable"
PKG_DIR="${OUT_DIR}/${PKG_NAME}"
ZIP_PATH="${OUT_DIR}/${PKG_NAME}.zip"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33mWARN: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# ---- 0. 环境检查 ---------------------------------------------------------
log "检查交叉编译环境"
[[ "$(uname -s)" == "Darwin" ]] || die "本脚本面向 macOS 主机；Windows 原生构建请见 docs/WINDOWS_BUILD.md"
export PATH="${HOME}/.cargo/bin:${PATH}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
if ! command -v cargo-xwin >/dev/null 2>&1 && ! cargo xwin --version >/dev/null 2>&1; then
  die "未找到 cargo-xwin。安装: cargo install cargo-xwin"
fi
if ! rustup target list --installed 2>/dev/null | grep -qx "${TARGET}"; then
  die "未安装 target ${TARGET}。安装: rustup target add ${TARGET}"
fi
if ! command -v clang-cl >/dev/null 2>&1 && ! xcrun --find clang-cl >/dev/null 2>&1; then
  warn "未找到 clang-cl；cargo-xwin 也能用自带 LLVM，若链接失败: brew install llvm && export PATH=\"$(brew --prefix llvm)/bin:$PATH\""
fi
for f in binaries/win/7z.exe binaries/win/7z.dll binaries/7z-LICENSE.txt binaries/mac/7zz; do
  [[ -e "${f}" ]] || die "缺少 ${f}（见 binaries/README.md）"
done
printf '  version: %s\n' "${VERSION}"
printf '  target : %s\n' "${TARGET}"

# ---- 1. 前端 dist --------------------------------------------------------
log "构建前端（pnpm --dir gui build）"
if [[ -f "${ROOT_DIR}/gui/pnpm-lock.yaml" ]]; then
  pnpm --dir "${ROOT_DIR}/gui" install --frozen-lockfile
else
  pnpm --dir "${ROOT_DIR}/gui" install
fi
pnpm --dir "${ROOT_DIR}/gui" build

# 图标/配置变更后强制 tauri-build 重新生成 Windows 资源
touch "${ROOT_DIR}/gui/src-tauri/build.rs"

# ---- 2. 交叉编译 GUI + CLI ----------------------------------------------
# 注意：--features custom-protocol 只属于 GUI crate（tauri 的 feature），不能和 CLI
# 放在同一条 -p 命令里（否则 cargo 报 CLI 没有该 feature），所以分两条命令。
log "交叉编译 GUI（unparanoia，--features custom-protocol）"
cargo xwin build --release --target "${TARGET}" -p upzip --features custom-protocol
log "交叉编译 CLI（upzip-cli，内嵌 7-Zip）"
cargo xwin build --release --target "${TARGET}" -p upzip-cli

EXE_DIR="${ROOT_DIR}/target/${TARGET}/release"
GUI_EXE="${EXE_DIR}/upzip.exe"
CLI_EXE="${EXE_DIR}/upzip-cli.exe"
[[ -f "${GUI_EXE}" ]] || die "未生成 ${GUI_EXE}"
[[ -f "${CLI_EXE}" ]] || die "未生成 ${CLI_EXE}"

# ---- 3. 组装便携包（单 exe） ---------------------------------------------
log "组装便携包 ${PKG_DIR}（单 exe，不附 sidecar 7z）"
rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}"
cp "${GUI_EXE}" "${PKG_DIR}/Upzip.exe"
cp "${CLI_EXE}" "${PKG_DIR}/upzip-cli.exe"
# 只放许可证；7z.exe/7z.dll 已通过 include_bytes! 内嵌进两个 exe，不再需要侧车文件。
cp binaries/7z-LICENSE.txt "${PKG_DIR}/7z-LICENSE.txt"

# 便携包 README（自包含，不依赖仓库）
cat > "${PKG_DIR}/README.txt" <<EOF
Upzip（智能批量解压）${VERSION} — Windows x64 便携版（单 exe）
================================================

用法
----
1. 解压后可直接运行 Upzip.exe；本包不依赖外部 7z.exe / 7z.dll，
   也可以把 exe 单独拷到任意目录使用。
2. 双击 Upzip.exe 打开 GUI；把 .enc / 压缩包拖进窗口，输入口令即可。
3. 命令行: upzip-cli.exe --help
   upzip-cli.exe doctor            # 自检（7-Zip / 配置目录 / 平台）
   upzip-cli.exe run 文件.enc -p 口令 -o 输出目录

7-Zip 内嵌说明（首次运行解压缓存）
----------------------------------
GUI 与 CLI 都已内嵌 7-Zip 25.01（LGPL v2.1+，含 unRAR 限制），不再附带
7z.exe / 7z.dll 侧车文件。首次运行时会自动把内嵌运行时解压到：

    %LOCALAPPDATA%\Upzip\embedded-7z\25.01-r1\\

之后复用该缓存。可用环境变量 UPZIP_7Z_CACHE_DIR 覆盖缓存根目录
（实际目录为 <该值>\25.01-r1）。缓存缺失或损坏时，删除该目录后重启即可重建。
许可证全文见同目录 7z-LICENSE.txt。

本目录文件
----------
- Upzip.exe          GUI 主程序（内嵌前端 + 7-Zip 25.01）
- upzip-cli.exe CLI（内嵌 7-Zip 25.01；detect/inspect/run/config/doctor/cleanup-staging）
- 7z-LICENSE.txt     7-Zip 许可证（LGPL v2.1+，含 unRAR 限制）
- README.txt         本文件
- SHA256SUMS         本目录文件校验和

要求
----
- Windows 10/11 x64；Windows 11 自带 WebView2 Runtime，Windows 10 需先安装
  https://developer.microsoft.com/microsoft-edge/webview2/ （Evergreen Runtime）。
- 不支持 32 位 Windows。免安装、不需要管理员权限、不写系统目录。

诚实边界
--------
- 本包在 macOS 上交叉编译，构建者未在真实 Windows 机器上运行验证；
  首次发布前请在 Windows 上跑一遍 upzip-cli.exe doctor 与真实样本解密。
- 未签 Authenticode；SmartScreen 可能提示"未知发布者"。
EOF

# 校验和
( cd "${PKG_DIR}" && shasum -a 256 Upzip.exe upzip-cli.exe 7z-LICENSE.txt README.txt > SHA256SUMS )

# ---- 4. 打包 zip --------------------------------------------------------
log "打包 zip（使用随附的 7zz）"
rm -f "${ZIP_PATH}"
( cd "${OUT_DIR}" && "${ROOT_DIR}/binaries/mac/7zz" a -tzip -mx=9 "${ZIP_PATH}" "${PKG_NAME}" >/dev/null )
[[ -f "${ZIP_PATH}" ]] || die "zip 创建失败"

# ---- 5. 产物清单 --------------------------------------------------------
ZIP_SIZE="$(wc -c < "${ZIP_PATH}" | tr -d ' ')"
ZIP_SHA="$(shasum -a 256 "${ZIP_PATH}" | awk '{print $1}')"
log "完成"
printf '  包目录 : %s\n' "${PKG_DIR#${ROOT_DIR}/}"
printf '  zip    : %s\n' "${ZIP_PATH#${ROOT_DIR}/}"
printf '  大小   : %s 字节\n' "${ZIP_SIZE}"
printf '  sha256 : %s\n' "${ZIP_SHA}"
printf '  GUI exe: %s 字节  sha256 %s\n' "$(wc -c < "${PKG_DIR}/Upzip.exe" | tr -d ' ')" "$(shasum -a 256 "${PKG_DIR}/Upzip.exe" | awk '{print $1}')"
printf '  CLI exe: %s 字节  sha256 %s\n' "$(wc -c < "${PKG_DIR}/upzip-cli.exe" | tr -d ' ')" "$(shasum -a 256 "${PKG_DIR}/upzip-cli.exe" | awk '{print $1}')"
printf '\n\033[1;33m提醒: 单 exe 已内嵌 7-Zip，但未在真实 Windows 上运行验证，也未做 Authenticode 签名（见 docs/WINDOWS_BUILD.md）。\033[0m\n'
