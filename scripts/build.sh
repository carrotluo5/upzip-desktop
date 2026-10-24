#!/usr/bin/env bash
#
# unparanoia — macOS (及 Linux) 一键构建脚本
#
# 流程: 环境检查 -> cargo test --workspace -> pnpm install -> tauri build -> 打印产物
# 用法: ./scripts/build.sh
#
# 说明:
#   - 本仓库是单一 Cargo workspace，gui/src-tauri 是 workspace 成员，因此构建产物
#     在仓库根目录的 target/release/ 下（不是 gui/src-tauri/target）。
#   - tauri build 会通过 beforeBuildCommand 自动执行 pnpm sync:binaries + pnpm build
#     （前端资源与 7-Zip 二进制会打进 .app）。
#   - 不签名、不公证；本机 build 出的 .app 只有 ad-hoc 签名，分发给别人前需要
#     Developer ID 签名 + notarize（见 docs/BUILD.md「签名与分发」）。
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33mWARN: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

human_size() {
  if command -v du >/dev/null 2>&1; then du -sh "$1" 2>/dev/null | cut -f1; else echo "?"; fi
}
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else echo "(no shasum)"; fi
}

# ---- 1. 环境检查 ---------------------------------------------------------
log "检查构建环境"
for cmd in cargo node pnpm; do
  command -v "${cmd}" >/dev/null 2>&1 || \
    die "未找到 ${cmd}。请安装 Rust (https://rustup.rs)、Node.js >= 20、pnpm 11"
done
printf '  cargo: %s\n' "$(cargo --version)"
printf '  node : %s\n' "$(node --version)"
printf '  pnpm : %s\n' "$(pnpm --version)"

if [[ "$(uname -s)" == "Darwin" ]]; then
  xcode-select -p >/dev/null 2>&1 || die "未检测到 Xcode Command Line Tools，请先执行: xcode-select --install"
  printf '  xcode: %s\n' "$(xcode-select -p)"
fi

[[ -d "${ROOT_DIR}/gui" ]] || die "找不到 gui/ 目录，请在仓库根目录运行本脚本"
[[ -f "${ROOT_DIR}/binaries/mac/7zz" ]] || die "缺少 binaries/mac/7zz（7-Zip 25.01，见 binaries/README.md）"

# ---- 2. Rust 测试（core + unpack-core + cli + gui 后端） -------------------
log "运行 Rust 测试（cargo test --workspace，包含 oracle 向量与引擎单测）"
cargo test --workspace

# ---- 3. 前端依赖 ---------------------------------------------------------
log "安装前端依赖（gui/）"
if [[ -f "${ROOT_DIR}/gui/pnpm-lock.yaml" ]]; then
  pnpm --dir "${ROOT_DIR}/gui" install --frozen-lockfile
else
  warn "缺少 gui/pnpm-lock.yaml，使用非冻结安装；建议提交锁文件以保证可重复构建"
  pnpm --dir "${ROOT_DIR}/gui" install
fi

# ---- 4. Tauri 打包 -------------------------------------------------------
# 注意：构建被中断/超时杀掉后，target/release/bundle/macos/ 可能残留半成品 dmg 与
# rw.*.dmg，导致下次 bundle_dmg.sh 报 "hdiutil: convert failed - 文件已经存在"
# （tauri 只显示 failed to run bundle_dmg.sh）。清理方法见 docs/BUILD.md §5/§7。
# 建议同一工作区串行构建，失败时先按该处清理再重试。
log "构建 GUI（tauri build，默认 app + dmg）"
pnpm --dir "${ROOT_DIR}/gui" tauri build

# ---- 5. CLI release 二进制 ----------------------------------------------
log "构建 CLI（release，产品交付件）"
cargo build --release -p unparanoia-cli

# ---- 6. 产物清单 ---------------------------------------------------------
BUNDLE_DIR="${ROOT_DIR}/target/release/bundle"
BIN_DIR="${ROOT_DIR}/target/release"

log "构建完成，产物清单"
found=0
if [[ -d "${BUNDLE_DIR}/dmg" ]]; then
  for artifact in "${BUNDLE_DIR}"/dmg/*.dmg; do
    [[ -e "${artifact}" ]] || continue
    found=1
    printf '  %8s  %s\n         sha256 %s\n' "$(human_size "${artifact}")" "${artifact#${ROOT_DIR}/}" "$(sha256_of "${artifact}")"
  done
fi
if [[ -d "${BUNDLE_DIR}/macos" ]]; then
  for artifact in "${BUNDLE_DIR}"/macos/*.app; do
    [[ -e "${artifact}" ]] || continue
    found=1
    printf '  %8s  %s\n' "$(human_size "${artifact}")" "${artifact#${ROOT_DIR}/}"
  done
fi
if [[ -x "${BIN_DIR}/unparanoia" ]]; then
  found=1
  printf '  %8s  %s\n         sha256 %s\n' "$(human_size "${BIN_DIR}/unparanoia")" \
    "target/release/unparanoia" "$(sha256_of "${BIN_DIR}/unparanoia")"
fi
if [[ -x "${BIN_DIR}/unparanoia-cli" ]]; then
  found=1
  printf '  %8s  %s\n         sha256 %s\n' "$(human_size "${BIN_DIR}/unparanoia-cli")" \
    "target/release/unparanoia-cli" "$(sha256_of "${BIN_DIR}/unparanoia-cli")"
fi
[[ "${found}" -eq 1 ]] || warn "未找到 bundle 产物，请检查上面的构建日志"

printf '\n\033[1;32m全部完成\033[0m\n'
printf '提示: macOS 上未签名 .app 首次打开需右键 ->「打开」；产物位于 target/release/bundle/ (workspace 根 target).\n'
