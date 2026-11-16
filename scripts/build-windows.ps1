<#
.SYNOPSIS
  unparanoia — 在 Windows 主机上原生构建（NSIS 安装包或单 exe 便携包）。

.DESCRIPTION
  流程: 环境检查 -> cargo test --workspace -> pnpm install -> 构建 -> 打印产物。
  需要: Rust (MSVC toolchain) + Visual Studio C++ Build Tools、Node.js >= 20、pnpm 11、
        WebView2 Runtime（Windows 11 自带；Windows 10 需安装 Evergreen Runtime）。

  说明:
    - 本仓库是单一 Cargo workspace，gui/src-tauri 是成员，产物在仓库根 target\release。
    - GUI 与 CLI 都已内嵌 7-Zip 25.01（include_bytes!），安装包/便携包不再依赖
      sidecar 7z.exe / 7z.dll。
    - GUI 的裸 cargo 构建必须带 --features custom-protocol（本脚本的 -Portable 路径已带），
      否则 exe 使用 devUrl http://localhost:1420，用户机器上会白屏。
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
# 7-Zip 二进制在编译期通过 include_bytes! 内嵌进 GUI/CLI，构建时必须存在。
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
if ($LASTEXITCODE -ne 0) { Fail "pnpm install 失败" }

Step "同步随附 7-Zip 资源（gui\src-tauri\binaries）"
pnpm --dir "$Root\gui" sync:binaries
if ($LASTEXITCODE -ne 0) { Fail "sync:binaries 失败（检查 binaries\ 下文件）" }

$ver = (Select-String -Path "$Root\Cargo.toml" -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $ver) { Fail "无法从 Cargo.toml 读取 workspace.package.version" }

if ($Portable) {
  # 原生 Windows 便携版：单 exe 路线，不依赖 NSIS。
  # 前端 dist 必须先构建：Tauri 的 generate_context!/tauri-build 在编译期把 dist 内嵌进 exe。
  Step "构建前端 dist"
  pnpm --dir "$Root\gui" build
  if ($LASTEXITCODE -ne 0) { Fail "pnpm build 失败" }

  Step "构建 GUI exe（cargo build --features custom-protocol）"
  cargo build --release -p upzip --features custom-protocol
  if ($LASTEXITCODE -ne 0) { Fail "GUI 构建失败" }

  Step "构建 CLI exe（内嵌 7-Zip）"
  cargo build --release -p upzip-cli
  if ($LASTEXITCODE -ne 0) { Fail "CLI 构建失败" }

  Step "组装单 exe 便携包（Upzip-$ver-win-x64-portable）"
  $pkgName = "Upzip-$ver-win-x64-portable"
  $pkgDir = "$Root\target\windows-portable\$pkgName"
  $zipPath = "$Root\target\windows-portable\$pkgName.zip"
  Remove-Item -Recurse -Force $pkgDir -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force -Path $pkgDir | Out-Null
  Copy-Item "$Root\target\release\upzip.exe" "$pkgDir\Upzip.exe"
  Copy-Item "$Root\target\release\upzip-cli.exe" "$pkgDir\upzip-cli.exe"
  Copy-Item "$Root\binaries\7z-LICENSE.txt" "$pkgDir\7z-LICENSE.txt"
  @(
    "Upzip（智能批量解压）$ver — Windows x64 便携版（单 exe）",
    "================================================",
    "",
    "用法：解压后直接运行 Upzip.exe；两个 exe 均已内嵌前端与 7-Zip 25.01。",
    "首次运行会自动把内嵌 7-Zip 解压到 %LOCALAPPDATA%\Upzip\embedded-7z\25.01-r1\，",
    "之后复用；可用环境变量 UPZIP_7Z_CACHE_DIR 覆盖缓存根目录。无需安装外部 7-Zip。",
    "",
    "命令行：upzip-cli.exe --help / doctor / run 文件.enc -p 口令 -o 输出目录",
    "",
    "要求：Windows 10/11 x64 + WebView2 Runtime（Win11 自带，Win10 需装 Evergreen）。",
    "未签 Authenticode，SmartScreen 可能提示未知发布者。"
  ) | Set-Content -Encoding UTF8 "$pkgDir\README.txt"
  $lines = foreach ($f in Get-ChildItem -Path $pkgDir -File | Where-Object { $_.Name -ne 'SHA256SUMS' } | Sort-Object Name) {
    "{0}  {1}" -f (Sha256 $f.FullName), $f.Name
  }
  $lines | Set-Content -Encoding ASCII "$pkgDir\SHA256SUMS"
  Remove-Item -Force $zipPath -ErrorAction SilentlyContinue
  Compress-Archive -Path "$pkgDir\*" -DestinationPath $zipPath
} else {
  Step "构建 GUI 安装包（tauri build --bundles nsis）"
  pnpm --dir "$Root\gui" tauri build --bundles nsis
  if ($LASTEXITCODE -ne 0) { Fail "tauri build 失败" }
}

Step "构建完成，产物清单"
$found = $false
if ($Portable) {
  foreach ($p in @($zipPath, "$pkgDir\Upzip.exe", "$pkgDir\upzip-cli.exe")) {
    if (Test-Path $p) {
      $found = $true
      $item = Get-Item $p
      Write-Host ("  {0,12:N0} B  {1}" -f $item.Length, $item.FullName.Substring($Root.Length + 1))
      if (-not $item.PSIsContainer) { Write-Host ("               sha256 {0}" -f (Sha256 $p)) }
    }
  }
  Write-Host "  包内文件: Upzip.exe / upzip-cli.exe / 7z-LICENSE.txt / README.txt / SHA256SUMS"
} else {
  $nsisDir = "$Root\target\release\bundle\nsis"
  if (Test-Path $nsisDir) {
    Get-ChildItem -Path $nsisDir -Filter *.exe | ForEach-Object {
      $found = $true
      Write-Host ("  {0,12:N0} B  {1}" -f $_.Length, $_.FullName.Substring($Root.Length + 1))
      Write-Host ("               sha256 {0}" -f (Sha256 $_.FullName))
    }
  }
  foreach ($name in @('upzip.exe','upzip-cli.exe')) {
    $p = Join-Path "$Root\target\release" $name
    if (Test-Path $p) {
      $found = $true
      Write-Host ("  {0,12:N0} B  {1}" -f (Get-Item $p).Length, "target\release\$name")
      Write-Host ("               sha256 {0}" -f (Sha256 $p))
    }
  }
}
if (-not $found) { Write-Host "WARN: 未找到构造产物，请检查构建日志" -ForegroundColor Yellow }

Write-Host ""
Write-Host "全部完成" -ForegroundColor Green
if ($Portable) {
  Write-Host "提示: 单 exe 便携包在 target\windows-portable\；未签名 exe 首次运行会被 SmartScreen 拦截。"
} else {
  Write-Host "提示: 未签名 exe 首次运行会被 SmartScreen 拦截；产物在 target\release\bundle\nsis。"
}
