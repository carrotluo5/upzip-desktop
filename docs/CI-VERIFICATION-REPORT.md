# CI 验收报告（GitHub Actions，main @ b5cc1eb）

- 验收时间：2026-11-17（Asia/Shanghai）
- 远端仓库：`https://github.com/carrotluo5/upzip-desktop.git`
- 验收对象：提交 `b5cc1eb81df452c460c32b78b3df8bd78ab441eb`（10 项优化 + 测试 + 验收文档）
- 结论：GitHub Actions run `44010090137` 的 `macOS (latest)` 与 `Windows (x64)` 两个 job 均为 `success`；`Publish GitHub Release` 因非 tag 按预期 `skipped`。本次为首次推送即通过，无失败重试。

> 本报告所有 SHA、Run ID、URL、时间、Artifact 名称/大小与日志摘要均取自本机实际执行的 `git`、`gh` 命令输出；未创建或推送任何 `v*` tag。报告按要求保留在本地工作区，未再次 push。

---

## 1. 推送与提交信息

### 1.1 推送前核查

| 项目 | 实际结果 |
|---|---|
| 分支 | `main`，跟踪 `origin/main` |
| 远端 | `origin  https://github.com/carrotluo5/upzip-desktop.git`（fetch/push 相同） |
| 推送前 HEAD | `777282e`（`ci: tag 自动发布 Release；Windows 包内 SHA256SUMS 改用 LF；文档补发版流程`） |
| 工作区 | 18 个已修改文件 + 3 个未跟踪新增文件；无 `target/`、`testdata/`、`gui/src-tauri/binaries/`、密钥或临时调试文件混入 |
| 改动规模 | `git diff --stat`：18 files changed，+941 / −152（不含新增文件） |
| 敏感串扫描 | `git diff` 中无 `ghp_`/`gho_`/私钥/`api_key`/`secret` 等模式（`NO_SECRET_PATTERNS`） |

### 1.2 提交

| 项目 | 实际结果 |
|---|---|
| Commit SHA | `b5cc1eb81df452c460c32b78b3df8bd78ab441eb` |
| Commit Message | `优化：完成 10 项解压/批处理/GUI 修复并补齐测试与验收文档` |
| 变更统计 | 21 files changed, 1283 insertions(+), 152 deletions(-)；新增 3 个文件 |
| 推送命令 | `git push origin main` |
| 推送输出 | `To https://github.com/carrotluo5/upzip-desktop.git` / `777282e..b5cc1eb  main -> main`（exit 0） |
| Tag | **未创建、未推送**任何 `v*` tag；仅触发 `main` 分支 push 构建 |

提交包含的 21 个文件：

| 状态 | 文件 |
|---|---|
| M | `crates/cli/src/main.rs` |
| M | `crates/core/src/decrypt.rs` |
| M | `crates/core/src/zip_extract.rs` |
| M | `crates/embedded-7z/src/lib.rs` |
| M | `crates/unpack-core/src/detect.rs` |
| M | `crates/unpack-core/src/engine.rs` |
| M | `crates/unpack-core/src/queue.rs` |
| M | `crates/unpack-core/src/sevenzip.rs` |
| M | `crates/unpack-core/src/split.rs` |
| M | `crates/unpack-core/src/util.rs` |
| A | `docs/OPTIMIZATION-REPORT.md` |
| M | `gui/package.json` |
| M | `gui/src-tauri/src/lib.rs` |
| M | `gui/src/App.tsx` |
| M | `gui/src/components/PasswordsTab.tsx` |
| A | `gui/src/task-logic.ts` |
| M | `gui/src/types.ts` |
| M | `gui/src/vite-env.d.ts` |
| A | `gui/tests/task-logic.test.ts` |
| M | `gui/tsconfig.node.json` |
| M | `gui/vite.config.ts` |

推送前（同一工作区最终代码）本地基线复跑结果：

| 命令 | 退出码 | 关键输出 |
|---|---|---|
| `pnpm --dir gui sync:binaries` | 0 | `sync-binaries: 0 updated, 0 missing source(s)` |
| `cargo test --workspace` | 0 | 128 passed / 0 failed / 2 ignored，0 warning |
| `pnpm --dir gui test:frontend` | 0 | 5 pass / 0 fail |
| `pnpm --dir gui build` | 0 | js 285.58 kB (gzip 86.93)、css 13.99 kB (gzip 3.37) |
| `./scripts/e2e-verify.sh` | 0 | `104 项检查全部通过` / `ALL E2E CHECKS PASSED` |

---

## 2. GitHub Actions 运行总览

| 项目 | 实际结果 |
|---|---|
| Workflow | `build`（`.github/workflows/build.yml`） |
| Run ID | `44010090137` |
| Run URL | https://github.com/carrotluo5/upzip-desktop/actions/runs/44010090137 |
| 触发事件 | `push`（自动触发，非 `workflow_dispatch`） |
| 分支 | `main` |
| Head SHA | `b5cc1eb81df452c460c32b78b3df8bd78ab441eb`（与本地 HEAD 一致） |
| 总状态 | `completed` / `conclusion = success` |
| 创建时间 | 2026-11-17T03:25:21Z（11:25:21 +08:00） |
| 结束时间 | 2026-11-17T03:30:31Z（11:30:31 +08:00） |
| 总耗时 | 约 5 分 10 秒（310 s，run 创建至结束） |
| Job 汇总 | `Windows (x64)` success；`macOS (latest)` success；`Publish GitHub Release` skipped |

Job 时间线（`gh run view 44010090137 --json jobs`）：

| Job | Job ID | 开始 (UTC) | 结束 (UTC) | 耗时 | 结论 |
|---|---|---|---|---|---|
| `Windows (x64)` | 133794752879 | 03:25:23 | 03:30:31 | 5m08s | success |
| `macOS (latest)` | 133794753275 | 03:25:23 | 03:29:07 | 3m44s | success |
| `Publish GitHub Release` | 133796737986 | 03:30:31 | 03:30:31 | 0s | skipped（`refs/tags/v*` 才运行） |

跟踪方式：`gh run watch 44010090137 --exit-status --interval 20`，退出码 0；随后用 `gh run view 44010090137 --json status,conclusion,jobs,url,headSha,createdAt,updatedAt,event` 复核。

---

## 3. 分 Job 详细验收记录

### 3.1 macOS (latest)

- Runner：`macos-latest`（arm64，日志中有 “Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times” 提示，未影响结果）。
- Job 时间：2026-11-17T03:25:23Z → 03:29:07Z（3m44s）。
- 16 个主步骤全部 `success`（另有 4 个 Post 步骤与 Complete job 亦为 success）；主步骤耗时如下（由 job 日志时间戳聚合）：

| # | 步骤 | 耗时（约） | 结论 |
|---|---|---|---|
| 1 | Set up job | 2.6s | success |
| 2 | Run actions/checkout@v4 | 1.5s | success |
| 3 | Setup Rust | 6.7s | success |
| 4 | Rust cache | 13.6s | success |
| 5 | Setup pnpm | 1.2s | success |
| 6 | Setup Node.js | 2.4s | success |
| 7 | Install frontend deps | 1.7s | success |
| 8 | Sync bundled 7-Zip resources | 0.4s | success |
| 9 | cargo test (workspace) | 13.2s | success |
| 10 | Build CLI (release) | 33.3s | success |
| 11 | Make e2e fixtures | 2.8s | success |
| 12 | Run CLI e2e | 6.4s | success |
| 13 | Build frontend | 1.9s | success |
| 14 | Build Tauri app (app + dmg) | 2m07s | success |
| 15 | Stage release assets | 0.7s | success |
| 16 | Upload macOS artifacts | 2.0s | success |

关键日志证据：

- `cargo test (workspace)`：日志 `test result:` 汇总 **128 passed / 0 failed / 2 ignored**；`warning:` 计数 0。新增的 `test util::tests::free_space_resolves_existing_dirs_and_future_children ... ok` 在 macOS 上通过（日志第 839 行）。
- `Run CLI e2e`：输出 `104 项检查全部通过` / `ALL E2E CHECKS PASSED`（日志 1085–1086 行）。
- `Build frontend` / Tauri 前置构建：`dist/assets/index-j9lH0oOW.css 13.99 kB (gzip 3.37)`、`dist/assets/index-NKt0h4J4.js 285.58 kB (gzip 86.93)`。
- `Build Tauri app (app + dmg)`：`Bundling Upzip.app`、`Bundling Upzip_0.4.0_aarch64.dmg`，最终 `Finished 2 bundles`。
- `Stage release assets` 的 `ls -l` 实际输出：

```
total 30808
-rw-r--r--  1 runner  staff  6203551 Nov 17 03:29 Upzip-0.4.0-macos-arm64.app.zip
-rw-r--r--@ 1 runner  staff  6526041 Nov 17 03:29 Upzip-0.4.0-macos-arm64.dmg
-rwxr-xr-x  1 runner  staff  3037232 Nov 17 03:29 upzip-cli-macos
```

Artifact（`gh api repos/carrotluo5/upzip-desktop/actions/runs/44010090137/artifacts`）：

| Artifact | Artifact ID | 大小 | 创建时间 (UTC) | 过期 | 内容（下载后实测） |
|---|---|---|---|---|---|
| `upzip-macos` | 14296441839 | 14,164,548 bytes（13.51 MiB） | 03:29:03 | 否 | `Upzip-0.4.0-macos-arm64.app.zip` 6,203,551 B；`Upzip-0.4.0-macos-arm64.dmg` 6,526,041 B；`upzip-cli-macos` 3,037,232 B |

下载命令：`gh run download 44010090137 -n upzip-macos -D /tmp/ci-artifacts/macos`。

### 3.2 Windows (x64)

- Runner：`windows-latest`（x64）。
- Job 时间：2026-11-17T03:25:23Z → 03:30:31Z（5m08s）。
- 16 个主步骤全部 `success`（另有 4 个 Post 步骤与 Complete job 亦为 success）；主步骤耗时如下：

| # | 步骤 | 耗时（约） | 结论 |
|---|---|---|---|
| 1 | Set up job | 1.3s | success |
| 2 | Run actions/checkout@v4 | 4.5s | success |
| 3 | Setup Rust | 18.9s | success |
| 4 | Rust cache | 26.5s | success |
| 5 | Setup pnpm | 1.8s | success |
| 6 | Setup Node.js | 7.5s | success |
| 7 | Install frontend deps | 2.8s | success |
| 8 | Sync bundled 7-Zip resources | 0.9s | success |
| 9 | cargo test (workspace) | 35.3s | success |
| 10 | Resolve version | <0.1s | success |
| 11 | Build frontend | 3.0s | success |
| 12 | Build CLI (release) | 39.0s | success |
| 13 | Build GUI (release, custom-protocol) | 2m38s | success |
| 14 | CLI smoke test | 0.3s | success |
| 15 | Package single-exe portable zip | ≈1.1s（含 zip 压缩） | success |
| 16 | Upload Windows portable artifacts | 1.7s | success |

重点核查项：

1. **`#[cfg(windows)] free_space`（`GetDiskFreeSpaceExW`）在真实 Windows 上编译并执行**：`cargo test (workspace)` 通过，`test result:` 汇总 **128 passed / 0 failed / 2 ignored**，`warning:` 计数 0；新增用例 `test util::tests::free_space_resolves_existing_dirs_and_future_children ... ok`（日志第 831 行）实际调用了 Win32 API 路径，而非被 `cfg` 跳过。
2. **CLI 冒烟**（`upzip-cli.exe --version` 与 `upzip-cli.exe doctor`）实际输出：

```
upzip-cli 0.4.0
平台：windows x86_64
配置目录：C:\Users\runneradmin\AppData\Roaming\upzip
配置文件：C:\Users\runneradmin\AppData\Roaming\upzip\config.json（不存在）
settings.json：C:\Users\runneradmin\AppData\Roaming\upzip\settings.json（不存在）
processed.json：C:\Users\runneradmin\AppData\Roaming\upzip\processed.json（不存在，0 条记录）
7-Zip：C:\Users\runneradmin\AppData\Local\Upzip\embedded-7z\25.01-r1\7z.exe（7-Zip 25.01 (x64) : Copyright (c) 1999-2025 Igor Pavlov : 2025-08-03，来源：内嵌（Windows 单 exe））
detect 自检：通过（kind=not_archive）
```

3. **GUI custom-protocol 构建**：`Build GUI (release, custom-protocol)` 步骤 success（耗时约 2m38s，用于 Rust 编译 + 打包），产物 `target/release/upzip.exe`。
4. **便携包**：`Package single-exe portable zip` 生成 `Upzip-0.4.0-win-x64-portable.zip`，并复制 `upzip.exe`、`upzip-cli.exe` 到 `release-assets/`；包内 README 说明两个 exe 均内嵌 7-Zip 25.01、要求 Windows 10/11 x64 + WebView2、未签 Authenticode；`SHA256SUMS` 使用 LF 换行。

Artifact：

| Artifact | Artifact ID | 大小 | 创建时间 (UTC) | 过期 | 内容（下载后实测） |
|---|---|---|---|---|---|
| `upzip-windows` | 14296561718 | 16,199,391 bytes（15.45 MiB） | 03:30:27 | 否 | `Upzip-0.4.0-win-x64-portable.zip` 8,131,855 B；`upzip.exe` 14,177,792 B；`upzip-cli.exe` 6,556,672 B |

下载命令：`gh run download 44010090137 -n upzip-windows -D /tmp/ci-artifacts/windows`。

---

## 4. 失败修复记录

本次无需失败修复：

- 首次推送 `b5cc1eb` 即触发 run `44010090137`，两个构建 job 一次通过（conclusion 均为 `success`），未出现失败 job，未使用 `gh workflow run` 手动兜底，也未修改代码后重推。
- `Publish GitHub Release` job 显示 `skipped` 属于预期：其 `if` 条件仅匹配 `refs/tags/v*`，本次按要求未创建 tag。
- 唯一的非失败提示是 GitHub Actions 的 Node 20 弃用注解：`actions/checkout@v4`、`actions/setup-node@v4`、`actions/upload-artifact@v4`、`pnpm/action-setup@v4` 被强制运行在 Node 24 上。该注解不影响 job 结论，但建议后续升级 action 版本以消除告警。

---

## 5. 诚实边界与未覆盖项

### 5.1 已由 GitHub Actions 真实 runner 验证

- **Windows x64**：Rust workspace 编译与全部单测（含 `#[cfg(windows)] free_space` 的 `GetDiskFreeSpaceExW` 路径实际执行）、前端 `tsc + vite build`、CLI release 构建、GUI `custom-protocol` release 构建、`upzip-cli.exe --version` / `doctor` 冒烟（内嵌 7-Zip 25.01 加载、配置目录解析、detect 自检）、便携 zip 打包与 artifact 上传。
- **macOS arm64**：Rust workspace 编译与全部单测、CLI release 构建、e2e fixture 与 `e2e-verify.sh`（104 项）、前端构建、Tauri `.app` + `.dmg` 打包、release assets 暂存与 artifact 上传。
- 两平台均确认：无编译 warning，无测试失败；`real_sample` 的 2 个用例外均执行（详见 5.3）。

### 5.2 仍未覆盖的真机项

- **Windows 桌面 GUI 实际启动与 WebView2 交互**：CI 只验证 `upzip.exe` 编译与打包，未在 Windows 上启动窗口、点击按钮或验证 WebView2 运行时行为。
- **macOS 未签名 `.app` / `.dmg` 实机双击**：CI 产物未做 codesign/notarize，未验证 Gatekeeper 拦截、首次打开提示与拖拽安装体验。
- **Windows 便携包解压后运行**：未在真实 Windows 桌面解压 `Upzip-0.4.0-win-x64-portable.zip` 并双击 `Upzip.exe`。
- **Windows 上的 e2e**：`e2e-verify.sh` 仅在 macOS job 执行，Windows job 未跑该脚本。
- **`free_space` 的 Windows 边缘分支**：单测覆盖现有目录与尚不存在的子路径；UNC 前缀、`\\?\` 长路径、无权限路径等分支未被真实 Windows 用例覆盖。
- **真实文档容器样本**：内层级联保留 Office/JAR 容器的测试使用合成 docx，未使用真实 Office 文档。
- **`real_sample.rs`**：2 个用例仍为 `ignored`，需要自备 `PARANOIA_SAMPLE_ENC` / `SHA256` / `PASSWORD` 真实加密样本才能执行。
- **性能**：未做吞吐/内存基准，仅验证正确性与构建产物。

### 5.3 环境与版本边界

- 两 job 的 Rust 版本、pnpm/Node 版本由 workflow 固定；本报告未单独记录 runner 镜像版本号。
- GitHub Actions 对 Node 20 action 的弃用注解见第 4 节，属工具链告警而非本次代码问题。
- Artifact 保留期以 GitHub 仓库设置为准（查询时 `expired = false`）。
