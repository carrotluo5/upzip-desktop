<#
.SYNOPSIS
  unparanoia — 在 Windows 主机上原生构建 GUI 安装包（NSIS）。

.DESCRIPTION
  流程: 环境检查 -> cargo test --workspace -> pnpm install -> tauri build (nsis) -> 打印产物。
  需要: Rust (MSVC toolchain) + Visual Studio C++ Build Tools、Node.js >= 20、pnpm 11、
        WebView2 Runtime（Windows 11 自带；Windows 10 需安装 Evergreen Runtime）。

  说明:
    - 本仓库是单一 Cargo workspace，gui/src-tauri 是成员，产物在仓库根 target\release\bundle\nsis。
    - tauri build 会通过 beforeBuildCommand 自动执行 pnpm sync:binaries + pnpm build，
      并把 binaries/win/7z.exe、7z.dll 打进安装包（见 gui/src-tauri/tauri.windows.conf.json）。
    - 未做 Authenticode 签名；分发给别人前需要签名证书（见 docs/WINDOWS_BUILD.md）。
    - 从 macOS/Linux 交叉构建便携版请用 scripts/build-windows-portable.sh。
#>
[CmdletBinding()]
param(
  [switch]$SkipTests,
  [switch]$Portable
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Step($msg) { Write-Host ""; Write-Host "==> $msg" -ForegroundColor Cyan }
function Fail($msg) { Write-Host "ERROR: $msg" -ForegroundColor Red; exit 1 }
function Sha256($path) { (Get-FileHash -Algorithm SHA256 -Path $path).Hash.ToLower() }

Step "检查构建环境"
foreach ($cmd in @('cargo','node','pnpm')) {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) { Fail "未找到 $cmd（Rust: https://rustup.rs，Node.js >= 20，pnpm 11）" }
}
Write-Host "  cargo: $((cargo --version) -join ' ')"
Write-Host "  node : $(node --version)"
Write-Host "  pnpm : $(pnpm --version)"

if (-not (Test-Path "$Root\gui")) { Fail "找不到 gui\ 目录，请在仓库根目录运行本脚本" }
if (-not (Test-Path "$Root\binaries\win\7z.exe")) { Fail "缺少 binaries\win\7z.exe（见 binaries/README.md）" }
if (-not (Test-Path "$Root\binaries\win\7z.dll")) { Fail "缺少 binaries\win\7z.dll" }

if (-not $SkipTests) {
  Step "运行 Rust 测试（cargo test --workspace）"
  cargo test --workspace
  if ($LASTEXITCODE -ne 0) { Fail "cargo test 失败" }
}

Step "安装前端依赖（gui\）"
if (Test-Path "$Root\gui\pnpm-lock.yaml") {
  pnpm --dir "$Root\gui" install --frozen-lockfile
} else {
  Write-Host "WARN: 缺少 gui\pnpm-lock.yaml，使用非冻结安装" -ForegroundColor Yellow
  pnpm --dir "$Root\gui" install
}

Step "构建 GUI（tauri build）"
if ($Portable) {
  # 原生 Windows 便携版：只构建 exe，不打包 NSIS；产物对应 scripts/build-windows-portable.sh 的 win 部分
  pnpm --dir "$Root\gui" tauri build --no-bundle
} else {
  pnpm --dir "$Root\gui" tauri build --bundles nsis
}
if ($LASTEXITCODE -ne 0) { Fail "tauri build 失败" }

Step "构建完成，产物清单"
$found = $false
$nsisDir = "$Root\target\release\bundle\nsis"
if (Test-Path $nsisDir) {
  Get-ChildItem -Path $nsisDir -Filter *.exe | ForEach-Object {
    $found = $true
    Write-Host ("  {0,10:N0} B  {1}" -f $_.Length, $_.FullName.Substring($Root.Length + 1))
    Write-Host ("             sha256 {0}" -f (Sha256 $_.FullName))
  }
}
$exeDir = "$Root\target\release"
foreach ($name in @('unparanoia.exe','unparanoia-cli.exe')) {
  $p = Join-Path $exeDir $name
  if (Test-Path $p) {
    $found = $true
    Write-Host ("  {0,10:N0} B  {1}" -f (Get-Item $p).Length, "target\release\$name")
    Write-Host ("             sha256 {0}" -f (Sha256 $p))
  }
}
if (-not $found) { Write-Host "WARN: 未找到 bundle 产物，请检查构建日志" -ForegroundColor Yellow }

Write-Host ""
Write-Host "全部完成" -ForegroundColor Green
Write-Host "提示: 未签名 exe 首次运行会被 SmartScreen 拦截；产物在 target\release\bundle\nsis。"
