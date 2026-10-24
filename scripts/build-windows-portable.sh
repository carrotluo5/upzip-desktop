#!/usr/bin/env bash
#
# unparanoia — 从 macOS 交叉构建 Windows x64 "便携版" zip
#
# 产出: target/windows-portable/Unparanoia-<version>-win-x64-portable.zip
#   内含 GUI exe + CLI exe + 7z.exe/7z.dll/7z-LICENSE.txt + README + SHA256SUMS
#
# 前置（一次性）:
#   brew install llvm
#   cargo install cargo-xwin
#   rustup target add x86_64-pc-windows-msvc
#   export PATH="$HOME/.cargo/bin:$PATH"     # Homebrew rust 默认不带 Windows target
#
# 说明:
#   - 这是"无安装器"的便携版：解压即用，Windows 11 自带 WebView2 Runtime。
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
PKG_NAME="Unparanoia-${VERSION}-win-x64-portable"
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
log "交叉编译 GUI（unparanoia）与 CLI（unparanoia-cli）"
cargo xwin build --release --target "${TARGET}" -p unparanoia -p unparanoia-cli

EXE_DIR="${ROOT_DIR}/target/${TARGET}/release"
GUI_EXE="${EXE_DIR}/unparanoia.exe"
CLI_EXE="${EXE_DIR}/unparanoia-cli.exe"
[[ -f "${GUI_EXE}" ]] || die "未生成 ${GUI_EXE}"
[[ -f "${CLI_EXE}" ]] || die "未生成 ${CLI_EXE}"

# ---- 3. 组装便携包 -------------------------------------------------------
log "组装便携包 ${PKG_DIR}"
rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}"
cp "${GUI_EXE}" "${PKG_DIR}/Unparanoia.exe"
cp "${CLI_EXE}" "${PKG_DIR}/unparanoia-cli.exe"
cp binaries/win/7z.exe "${PKG_DIR}/7z.exe"
cp binaries/win/7z.dll "${PKG_DIR}/7z.dll"
cp binaries/7z-LICENSE.txt "${PKG_DIR}/7z-LICENSE.txt"

# 便携包 README（自包含，不依赖仓库）
cat > "${PKG_DIR}/README.txt" <<EOF
Unparanoia（帕罗诺亚解包器）${VERSION} — Windows x64 便携版
================================================

用法
----
1. 解压整个目录（不要只拷 exe）。
2. 双击 Unparanoia.exe 打开 GUI；把 .enc / 压缩包拖进窗口，输入口令即可。
3. 命令行: unparanoia-cli.exe --help
   unparanoia-cli.exe doctor            # 自检（7-Zip 路径 / 配置目录）
   unparanoia-cli.exe run 文件.enc -p 口令 -o 输出目录

本目录文件
----------
- Unparanoia.exe  GUI 主程序
- unparanoia-cli.exe    CLI（detect/inspect/run/config/doctor/cleanup-staging）
- 7z.exe / 7z.dll         随附的 7-Zip 25.01（子进程调用，不注入进程）
- 7z-LICENSE.txt          7-Zip 许可证（LGPL v2.1+，含 unRAR 限制）
- SHA256SUMS              本目录文件校验和

要求
----
- Windows 10/11 x64；Windows 11 自带 WebView2 Runtime，Windows 10 需先安装
  https://developer.microsoft.com/microsoft-edge/webview2/ （Evergreen Runtime）。
- 不支持 32 位 Windows。便携版不需要管理员权限、不写系统目录。

诚实边界
--------
- 本包在 macOS 上交叉编译，构建者未在真实 Windows 机器上运行验证；
  首次发布前请在 Windows 上跑一遍 \`unparanoia-cli.exe doctor\` 与真实样本解密。
- 未签 Authenticode；SmartScreen 可能提示"未知发布者"。
EOF

# 校验和
( cd "${PKG_DIR}" && shasum -a 256 Unparanoia.exe unparanoia-cli.exe 7z.exe 7z.dll 7z-LICENSE.txt README.txt > SHA256SUMS )

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
printf '  GUI exe: %s 字节  sha256 %s\n' "$(wc -c < "${PKG_DIR}/Unparanoia.exe" | tr -d ' ')" "$(shasum -a 256 "${PKG_DIR}/Unparanoia.exe" | awk '{print $1}')"
printf '  CLI exe: %s 字节  sha256 %s\n' "$(wc -c < "${PKG_DIR}/unparanoia-cli.exe" | tr -d ' ')" "$(shasum -a 256 "${PKG_DIR}/unparanoia-cli.exe" | awk '{print $1}')"
printf '\n\033[1;33m提醒: 未在真实 Windows 上运行验证，也未做 Authenticode 签名（见 docs/WINDOWS_BUILD.md）。\033[0m\n'
