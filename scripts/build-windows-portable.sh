#!/usr/bin/env bash
#
# paranoia-enc-decryptor — Windows x64 单文件便携版交叉构建脚本
# （在 macOS 上交叉编译；Linux 亦可，但本仓库仅在 macOS 实测）
#
# 流程: 环境检查 -> cargo-xwin 交叉编译 GUI -> 打印 exe 路径 / 大小 / SHA-256
# 用法: ./scripts/build-windows-portable.sh
#
# 前置依赖（macOS）:
#   rustup (stable toolchain) + rustup target add --toolchain stable x86_64-pc-windows-msvc
#   brew install llvm          # 提供 clang-cl / lld-link（cargo-xwin 使用）
#   cargo install cargo-xwin   # 首次运行会联网下载 MSVC CRT/SDK（约 1.1 GB 缓存）
#
# 说明:
#   - 产出为单文件便携 exe：Tauri 的 Rust 后端与静态前端资源全部内嵌在 exe 中，
#     无需安装程序、无需随附 DLL/目录；运行时仅依赖 Windows 10+ 自带的 UCRT
#     与系统 WebView2 Runtime（Windows 11 自带，Windows 10 通常随 Edge/系统更新安装）。
#   - 本脚本不能在 macOS 上生成 Windows NSIS 安装包（那需要真正的 Windows 环境，
#     见 .github/workflows/build.yml 的 windows-latest 任务或 docs/WINDOWS_BUILD.md）。
#   - 这是交叉编译产物，未在真实 Windows 上运行验证；请以真机结果为准。
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33mWARN: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# ---- 1. 环境检查 ---------------------------------------------------------
log "检查交叉编译环境"

# 必须使用 rustup 的 stable 工具链（Homebrew 等发行版 rustc 的 sysroot 缺少 Windows std）
if ! command -v rustup >/dev/null 2>&1; then
  die "未找到 rustup。请从 https://rustup.rs 安装 Rust 工具链"
fi
export PATH="${HOME}/.cargo/bin:${PATH}"
export RUSTUP_TOOLCHAIN="stable"

if ! command -v cargo >/dev/null 2>&1; then
  die "未找到 cargo"
fi
printf '  cargo  : %s\n' "$(cargo --version)"
printf '  rustc  : %s\n' "$(rustc --version)"

if ! cargo xwin --version >/dev/null 2>&1; then
  die "未找到 cargo-xwin。请执行: cargo install cargo-xwin"
fi
printf '  cargo-xwin: %s\n' "$(cargo xwin --version 2>/dev/null | head -n 1)"

if ! rustup target list --installed --toolchain stable 2>/dev/null | grep -q '^x86_64-pc-windows-msvc$'; then
  die "rustup stable 工具链缺少 Windows target。请执行: rustup target add --toolchain stable x86_64-pc-windows-msvc"
fi

if ! command -v clang-cl >/dev/null 2>&1; then
  warn "未在 PATH 中找到 clang-cl。cargo-xwin 通常需要 LLVM（macOS: brew install llvm），"
  warn "或已自行配置。若链接阶段报找不到编译器/链接器，请先安装 LLVM。"
fi

if [[ ! -d "${ROOT_DIR}/gui/src-tauri" ]]; then
  die "找不到 gui/src-tauri 目录，请在仓库根目录运行本脚本"
fi

# 防止外部 RUSTFLAGS 干扰（静态 CRT 与 Tauri 的 webview2 静态库不兼容，见 docs/WINDOWS_BUILD.md §5.4）
if [[ -n "${RUSTFLAGS:-}" ]]; then
  warn "已忽略外部 RUSTFLAGS=${RUSTFLAGS}（使用默认动态 CRT 配置）"
  unset RUSTFLAGS
fi

# ---- 2. 交叉编译 ---------------------------------------------------------
log "交叉编译 Windows x64 GUI（单文件便携 exe）"
cd "${ROOT_DIR}/gui/src-tauri"
cargo xwin build --release --target x86_64-pc-windows-msvc

EXE="${ROOT_DIR}/gui/src-tauri/target/x86_64-pc-windows-msvc/release/paranoia-enc-decryptor.exe"
[[ -f "${EXE}" ]] || die "未找到构建产物: ${EXE}"

# ---- 3. 产物信息 ---------------------------------------------------------
log "构建完成"
printf '  path  : %s\n' "${EXE#${ROOT_DIR}/}"
printf '  size  : %s bytes\n' "$(wc -c < "${EXE}" | tr -d ' ')"
if command -v file >/dev/null 2>&1; then
  printf '  file  : %s\n' "$(file -b "${EXE}")"
fi
if command -v shasum >/dev/null 2>&1; then
  printf '  sha256: %s\n' "$(shasum -a 256 "${EXE}" | cut -d' ' -f1)"
fi

printf '\n\033[1;32m✅ 完成\033[0m\n'
printf '提示: 该 exe 为单文件便携版，可复制到 Windows 10/11 x64 直接双击运行；\n'
printf '      未在真实 Windows 上验证，运行时需系统 WebView2 Runtime（详见 docs/WINDOWS_BUILD.md）。\n'
