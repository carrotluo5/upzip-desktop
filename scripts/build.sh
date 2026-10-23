#!/usr/bin/env bash
#
# paranoia-enc-decryptor — macOS / Linux 一键构建脚本
#
# 流程: 环境检查 -> Rust 测试 -> 安装 GUI 依赖 -> Tauri 打包 -> 打印产物
# 用法: ./scripts/build.sh
#
# macOS 依赖: Rust stable (rustup) + Xcode Command Line Tools + Node.js >= 20 + pnpm
# Linux 依赖: Rust stable + Node.js >= 20 + pnpm + Tauri 系统依赖
#            (如 libwebkit2gtk-4.1-dev / libgtk-3-dev / libayatana-appindicator3-dev 等)
#
set -euo pipefail

# ---- 路径与日志 ----------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33mWARN: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

human_size() {
  if command -v du >/dev/null 2>&1; then
    du -sh "$1" 2>/dev/null | cut -f1
  else
    echo "?"
  fi
}

# ---- 1. 环境检查 ---------------------------------------------------------
log "检查构建环境"
for cmd in cargo node pnpm; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    die "未找到 ${cmd}。请安装: Rust (https://rustup.rs)、Node.js >= 20、pnpm (corepack enable 或 npm i -g pnpm)"
  fi
done
printf '  cargo: %s\n' "$(cargo --version)"
printf '  node : %s\n' "$(node --version)"
printf '  pnpm : %s\n' "$(pnpm --version)"

if [[ "$(uname -s)" == "Darwin" ]]; then
  if ! xcode-select -p >/dev/null 2>&1; then
    die "未检测到 Xcode Command Line Tools，请先执行: xcode-select --install"
  fi
  printf '  xcode: %s\n' "$(xcode-select -p)"
fi

if [[ ! -d "${ROOT_DIR}/gui" ]]; then
  die "找不到 gui/ 目录，请在仓库根目录运行本脚本"
fi

# ---- 2. Rust 测试 --------------------------------------------------------
log "运行 Rust 测试（workspace: core + cli）"
cargo test --workspace

# ---- 3. GUI 依赖 ---------------------------------------------------------
log "安装 GUI 依赖（gui/）"
cd "${ROOT_DIR}/gui"
if [[ -f pnpm-lock.yaml ]]; then
  pnpm install --frozen-lockfile
else
  warn "缺少 gui/pnpm-lock.yaml，使用非冻结安装（建议提交锁文件以保证可重复构建）"
  pnpm install
fi

# ---- 4. Tauri 打包 -------------------------------------------------------
log "构建 GUI（Tauri bundle）"
pnpm tauri build

# ---- 5. 产物 -------------------------------------------------------------
cd "${ROOT_DIR}"
BUNDLE_DIR="${ROOT_DIR}/gui/src-tauri/target/release/bundle"
BIN_DIR="${ROOT_DIR}/gui/src-tauri/target/release"

log "构建完成，产物清单"
found=0
if [[ -d "${BUNDLE_DIR}" ]]; then
  while IFS= read -r -d '' artifact; do
    found=1
    printf '  %8s  %s\n' "$(human_size "${artifact}")" "${artifact#${ROOT_DIR}/}"
  done < <(find "${BUNDLE_DIR}" -maxdepth 2 \
      \( -name '*.app' -o -name '*.dmg' -o -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*.exe' -o -name '*.msi' \) \
      -print0 2>/dev/null | sort -z)
fi
if [[ -f "${BIN_DIR}/paranoia-enc-decryptor" ]]; then
  found=1
  printf '  %8s  %s\n' "$(human_size "${BIN_DIR}/paranoia-enc-decryptor")" "gui/src-tauri/target/release/paranoia-enc-decryptor"
fi
if [[ "${found}" -eq 0 ]]; then
  warn "未找到 bundle 产物，请检查上面的构建日志"
fi

printf '\n\033[1;32m✅ 全部完成\033[0m\n'
printf '提示: macOS 上未签名 App 首次打开需右键 ->「打开」；产物位于 gui/src-tauri/target/release/\n'
