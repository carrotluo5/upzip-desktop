# paranoia-enc-decryptor — Windows x64 一键构建脚本
#
# 流程: 环境检查 -> Rust 测试 -> 安装 GUI 依赖 -> Tauri 打包 (NSIS) -> 打印产物
# 用法: powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
#
# 依赖:
#   - Rust stable (rustup, MSVC toolchain)  https://rustup.rs
#   - Node.js >= 20 + pnpm
#   - Visual Studio Build Tools（C++ 桌面开发工作负载）
#   - WebView2 Runtime（Windows 10/11 通常已预装）
#
#Requires -Version 5.1
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RootDir = Split-Path -Parent $PSScriptRoot
Set-Location $RootDir

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Fail([string]$Message) {
    Write-Host "ERROR: $Message" -ForegroundColor Red
    exit 1
}

# ---- 1. 环境检查 ---------------------------------------------------------
Write-Step "检查构建环境"
foreach ($cmd in @("cargo", "node", "pnpm")) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
        Fail "未找到 $cmd。请安装: Rust (https://rustup.rs)、Node.js >= 20、pnpm (corepack enable 或 npm i -g pnpm)"
    }
}
Write-Host "  cargo: $(cargo --version)"
Write-Host "  node : $(node --version)"
Write-Host "  pnpm : $(pnpm --version)"

if (-not [Environment]::Is64BitOperatingSystem) {
    Fail "本项目仅支持 64 位 Windows（x64）"
}

if (-not (Test-Path (Join-Path $RootDir "gui"))) {
    Fail "找不到 gui 目录，请在仓库根目录运行本脚本"
}

# ---- 2. Rust 测试 --------------------------------------------------------
Write-Step "运行 Rust 测试（workspace: core + cli）"
cargo test --workspace
if ($LASTEXITCODE -ne 0) { Fail "cargo test 失败（exit code $LASTEXITCODE）" }

# ---- 3. GUI 依赖 ---------------------------------------------------------
Write-Step "安装 GUI 依赖（gui）"
Set-Location (Join-Path $RootDir "gui")
$lockfile = Join-Path (Get-Location) "pnpm-lock.yaml"
if (Test-Path $lockfile) {
    pnpm install --frozen-lockfile
} else {
    Write-Host "WARN: 缺少 gui/pnpm-lock.yaml，使用非冻结安装（建议提交锁文件）" -ForegroundColor Yellow
    pnpm install
}
if ($LASTEXITCODE -ne 0) { Fail "pnpm install 失败（exit code $LASTEXITCODE）" }

# ---- 4. Tauri 打包（NSIS 安装包） ---------------------------------------
Write-Step "构建 GUI（Tauri, NSIS 安装包）"
pnpm tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) { Fail "pnpm tauri build 失败（exit code $LASTEXITCODE）" }

# ---- 5. 产物 -------------------------------------------------------------
Set-Location $RootDir
Write-Step "构建完成，产物清单"

$artifactCandidates = @()
$nsisDir = Join-Path $RootDir "gui\src-tauri\target\release\bundle\nsis"
if (Test-Path $nsisDir) {
    $artifactCandidates += @(Get-ChildItem $nsisDir -File | Where-Object { $_.Extension -in @(".exe", ".msi") })
}
$bareExe = Join-Path $RootDir "gui\src-tauri\target\release\paranoia-enc-decryptor.exe"
if (Test-Path $bareExe) { $artifactCandidates += Get-Item $bareExe }

if ($artifactCandidates.Count -eq 0) {
    Write-Host "WARN: 未找到 bundle 产物，请检查上面的构建日志" -ForegroundColor Yellow
} else {
    foreach ($artifact in $artifactCandidates) {
        $sizeMb = [math]::Round($artifact.Length / 1MB, 2)
        Write-Host ("  {0,8} MB  {1}" -f $sizeMb, $artifact.FullName)
    }
}

Write-Host ""
Write-Host "[OK] 全部完成" -ForegroundColor Green
Write-Host "提示: 未签名安装包会触发 SmartScreen，需「更多信息 -> 仍要运行」。产物位于 gui\src-tauri\target\release\"
