# 验证报告（Verification Report）

本文件记录 `paranoia-enc-decryptor` 首期交付（AES-256 `.enc` 解密）的验收证据：测试结果、真实样本哈希、批量与异常路径验证、独立 Java Oracle 交叉验证、macOS arm64 构建与 Windows 构建证据。v0.1.1 的修复与重新构建记录见 §7。

> 样本文件与明文输出**不入库**。本报告中的哈希均由开发机现场计算，任何人可用同样的样本与命令复现。

---

## 1. 环境

| 项目 | 值 |
| --- | --- |
| 主机 | macOS 27.2（Darwin arm64，Apple Silicon） |
| CPU 架构 | arm64（原生，非 Rosetta） |
| Rust | rustc 1.99.0（Homebrew）、cargo 1.99.0 |
| Node / pnpm | Node v26.10.0 / pnpm 11.27.0 |
| Xcode CLT | Command Line Tools（clang 21） |
| Tauri CLI | @tauri-apps/cli 2.x（见 `gui/package.json`） |
| 独立校验 JDK | Temurin 21.0.13+9（仅用于 Java Oracle，非交付依赖） |
| 样本 | `3138.7z.001.enc`（209,779,415 B）、`3138.7z.002.enc`（60,527,110 B），密码均为 `123` |

原始样本只读，未修改；下表中的 `.enc` SHA-256 是**加密文件本身**的哈希，用于证明样本未被改动。

| 样本 | `.enc` SHA-256（加密文件） | 解密后明文 SHA-256 | 明文大小 |
| --- | --- | --- | --- |
| `3138.7z.001.enc` | `078386d3ad550d438dcaa2f6297e5b75112e4fbaea6731685d56bc7e8254877c` | `9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c` | 209,715,200 B |
| `3138.7z.002.enc` | `42ef9720fef2b13cea7548be26261ce0b29df07aaddeb06208a70fbf7c9c9623` | `46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840` | 60,508,410 B |

两行明文的 SHA-256 与 Paranoia 原软件解出的 `3138.7z.001` / `3138.7z.002` 逐字节一致（独立 Java Oracle 用 `cmp` 复核，见 §5）。

---

## 2. 自动化测试

命令：

```bash
cargo test --workspace
```

结果（记录于最终验收运行）：

| 测试目标 | 结果 |
| --- | --- |
| `paranoia-enc-core` 单元测试 | **35 passed**, 0 failed |
| `tests/oracle_vectors.rs`（Rust ↔ Java Oracle 向量） | **1 passed** |
| `tests/real_sample.rs`（真实样本，默认 `#[ignore]`） | 2 ignored（见 §3 手动启用记录） |
| doc-tests / CLI | 0（CLI 无单元测试，行为由 E2E 覆盖） |

覆盖点（对应验收要求）：

- 文件头解析：magic / version / algorithm / customParams / salt 长度与偏移（`format.rs`）。
- 密钥派生：`password_to_bytes` 码点规则、HKDF-Skein1024→Argon2id→HKDF-SHA3-512 全链，含样本 001 的已知向量（`kdf.rs`）。
- 解密正确性：合成容器 round-trip、STORED / DEFLATED 内层 ZIP、CRC32、文件名恢复（`decrypt.rs`）。
- 错误密码：checkcode 校验失败 → `wrongPassword`，且不留下临时/半成品文件（`decrypt.rs`）。
- 输出路径解析：默认目录 = 首个待解密文件所在目录；自定义目录覆盖；重名策略 `rename` / `overwrite` / `error`（`output.rs`、`batch.rs`）。
- 密码记忆：读写、清除、路径解析（`config.rs`，使用临时目录避免污染真实配置）。
- 批量隔离：单文件失败不影响其余文件，计数与输出目录正确（`batch.rs`）。

真实样本集成测试（需提供本地样本，默认忽略）：

```bash
PARANOIA_SAMPLE_ENC=/path/to/3138.7z.002.enc \
PARANOIA_SAMPLE_SHA256=46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840 \
PARANOIA_SAMPLE_PASSWORD=123 \
cargo test -p paranoia-enc-core --test real_sample -- --ignored --nocapture
```

结果：2 passed。其中批量测试准备了 2 个正确密码副本 + 1 个被翻转首字节密码错误的副本，断言 2 成功 / 1 失败、失败原因为 `wrongPassword`、无残留输出。样本解密后 SHA-256 与期望值一致，样本本身未被修改。

---

## 3. 命令行端到端（核心逻辑）

所有 E2E 均在样本的 **/tmp 副本**上进行，原始 `/Volumes/Carrot/Download/*.enc` 只读未改动。

### 3.1 单文件 + 哈希比对

```bash
cargo run --release -p paranoia-enc-cli -- /tmp/e2e/inputs/3138.7z.002.enc -p 123
shasum -a 256 /tmp/e2e/inputs/3138.7z.002
```

输出（节选）：

```
OK   3138.7z.002
successes 1 failures 0
46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840  /tmp/e2e/inputs/3138.7z.002
```

- 输出文件大小 60,508,410 B，与原始明文一致；哈希与 §1 表中期望值一致。
- 输出目录默认 = 输入文件所在目录；原始文件名与扩展名 `3138.7z.002` 从文件头元数据（内层 ZIP entry 名）恢复。
- 耗时约 0.45 s（含 Argon2id 派生，60 MB 样本，Apple Silicon）。

### 3.2 文件头检查（--inspect）

```bash
cargo run --release -p paranoia-enc-cli -- --inspect /tmp/e2e/inputs/3138.7z.002.enc
```

关键字段：magic `SSEFE`，version 4，algorithm 0（AES-256），customParams `0x11`（Argon2id t=20，m=20480 KiB，p=4），salt
`f0131e597c16317aeb749495f04593df2b0ac15a4e2f0768c7a493f7abe7e210`。

### 3.3 批量 + 失败隔离 + 默认输出目录

```bash
# 2 个正确副本 + 1 个被截断到 30,000,000 B 的副本；不传 -o，验证默认输出目录
cargo run --release -p paranoia-enc-cli -- \
  /tmp/e2e2/inputs/3138.7z.002.enc \
  /tmp/e2e2/inputs/3138.7z.002-copy.enc \
  /tmp/e2e2/inputs/truncated.enc -p 123
```

结果：successes 2 / failures 1，退出码 1；两个成功输出的 SHA-256 均为 `46abe32c…d04ce840`；默认输出目录为**首个文件所在目录** `/tmp/e2e2/inputs`；第二个成功文件因重名按默认 `rename` 策略写为 `3138.7z (1).002`；截断文件报
`file integrity check failed (BLAKE3 MAC mismatch): the file is corrupted or was modified (integrity)`；无临时文件或半成品残留。

### 3.4 异常路径

| 场景 | 结果 |
| --- | --- |
| 密码错误 | `incorrect password (wrongPassword)`，退出码 1，输出目录为空（无半成品） |
| 文件截断 / 篡改 | `integrity`（BLAKE3 MAC 不匹配），不会写出文件 |
| 重名 + `--conflict error` | `output file already exists: …/3138.7z.002 (outputExists)` |
| 同目录重复解密（默认 `rename`） | 生成 `3138.7z (1).002`，reported original_name 仍为 `3138.7z.002` |
| `--json` | 输出机器可读 `{outcomes, successes, failures, outputDir}`，字段为 camelCase |
| 密码记忆 CLI | `--config-path` / `--clear-password` 可用；CLI 不会写入密码（记忆仅 GUI 提供） |

### 3.5 性能与体积（CLI）

- 释放版 CLI：`target/release/paranoia-enc-cli` 约 **750,624 B**（0.72 MiB），启动到输出 `--version` 约 0.00 s。
- 60 MB 样本解密约 0.45 s；209 MB 样本同量级线性增长（Argon2id 约 0.3 s 固定成本）。

---

## 4. 真实样本逐字节验证汇总

| 样本 | CLI 解密 SHA-256 | 期望 SHA-256 | 结论 |
| --- | --- | --- | --- |
| 001 | `9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c` | 同左 | ✅ 逐字节一致 |
| 002 | `46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840` | 同左 | ✅ 逐字节一致 |

---

## 5. 独立 Java Oracle 交叉验证

为避免“自己验证自己”，仓库内包含一个**非 Rust** 的独立校验收具（`verification/oracle/`，Java 21），其中容器布局、KDF 接线、AES-256-CTR 语义、BLAKE3 keyed MAC、checkcode 规则与内层 ZIP/CRC/SHA-256 逻辑均依据 `docs/FORMAT.md` 与官方源码独立重写，加密原语只读复用官方 vendored 代码；**未参考 Rust 实现**。

复现：

```bash
bash verification/oracle/test.sh
# 结尾：ALL CHECKS PASSED (MAC, checkcode, CRC, SHA-256, cmp, wrong-password)
```

结果：

- 两个样本 MAC 校验通过、checkcode 合法、内层 ZIP CRC32 匹配、解密 SHA-256 匹配、`cmp` 与原始明文**逐字节相同**。
- 错误密码 `456` 被 checkcode 规则拒绝（`wrongPassword`）。
- CTR 交叉验证：官方 `SICBlockCipher` 与 JDK AES/ECB 生成的 keystream 解密结果逐字节一致。
- 完整 KDF 中间值（l0PWHashV3 / Argon2 输出 / encKey / nonce / authKey / MAC）存于 `verification/oracle/test-vectors.json`，并由 Rust 集成测试 `crates/core/tests/oracle_vectors.rs` 在默认 `cargo test` 中逐项比对。
- 完整报告：[docs/oracle-report.md](oracle-report.md)。

---

## 6. GUI 验证（macOS arm64）

构建产物（v0.1.1）：`gui/src-tauri/target/release/bundle/macos/Paranoia Decryptor.app`（5.8 MiB，内含 `Contents/MacOS/paranoia-enc-decryptor` 4,888,448 B，Mach-O arm64）。用 `open ./Paranoia Decryptor.app` 启动，进程与窗口正常创建（已实际启动验证后退出）。

验证方式：release 二进制内置验收开关 `--autotest-spec <spec.json> --autotest-report <report.json>`，由真实前端代码驱动「添加文件 → 校验 → 开始解密 → 汇总」的完整数据路径，并在隔离配置目录（`PARANOIA_ENC_CONFIG_DIR=/tmp/guiverify2/config`）下运行；输入为样本副本（`sample-a.enc`、`sample-b.enc` 均为 `3138.7z.002.enc` 副本，另有首字节损坏的 `bad-password.enc`、`notes.txt` 与 5 个伪造头文件）。

| 验收项 | 结果 |
| --- | --- |
| 批量添加 + 同一文件去重 | 5 条输入中重复的 `sample-a.enc` 在路径去重阶段合并为 4 个唯一路径；列表无重复项 |
| 非 `.enc` 文件拒绝 | `notes.txt` 被拒绝加入，原因 `notEncExtension`（“不是 .enc 文件”），其余 3 个有效文件正常入列 |
| 默认输出目录 = 首个文件所在目录 | 不选目录时输出到 `/tmp/guiverify2/inputs`（首个添加文件父目录），界面显示该路径 |
| 文件名恢复 | 输出名为 `3138.7z.002`（来自容器内层 ZIP entry 名） |
| 批量解密 + 失败隔离 | 3 个有效文件：成功 2 / 失败 1；失败文件 `bad-password.enc` 报 `wrongPassword`（“incorrect password”），不影响其余文件；两成功输出 SHA-256 均 `46abe32c…d04ce840` |
| 整体/单文件进度 | 收到 8 次 `batch-progress` 事件与 3 次 `file-finished` 事件；界面整体进度 100%，计数 2 成功 / 1 失败 / 共 3；每文件行状态为“成功/成功/密码错误” |
| 物理拖拽（真实 OS 事件） | 用 Swift + Accessibility API 定位 Finder 中 `002.enc` 图标（AXPosition），CGEvent `mouseDown → 40×leftMouseDragged → mouseUp` 拖入 Tauri 窗口：`dropped=true`，`rawEventCount=56` 条真实 `enter/over/drop` 事件（首条 `enter` 携带 `paths=["/private/tmp/gui-test/002.enc"]`），文件成功入列（`fileCount=1`，60527110 B），无错误；见 `/tmp/gui-test/reports/dragdrop-fast.json`、脚本 `axfind.swift`/`drag.swift` |
| 重复添加同一文件的提示 | 同一路径分两次 `addPaths`：第一次 `accepted=1`，第二次 `duplicates=1, accepted=0`，列表保持 1 个文件，toast 文案“文件已在列表中”（`batch-dedupe.json`） |
| 密码错误不留半成品 | 失败文件无输出、无临时文件残留 |
| 重名策略 `rename`（默认） | 第二个同内容输出自动改名为 `3138.7z (1).002`，原始文件名报告仍为 `3138.7z.002` |
| 重名策略 `error` | 目标已存在时该文件失败，错误 `outputExists`（“output file already exists: …/3138.7z.002”），界面行状态“输出文件已存在” |
| 重名策略 `overwrite` | 覆盖已有文件并成功，输出 SHA-256 与期望一致 |
| 自定义输出目录 | 选择 `/tmp/guiverify2/custom-out` 后按该目录输出，界面显示为自定义目录 |
| 记住密码 + 重启自动填充 | 勾选“记住密码”并完成一次批量后，`config.json` 为 `{"remembered_password":"123"}`；**新进程**启动时密码框自动填充、勾选框保持勾选（`autoFilled=true`，`passwordLength=3`） |
| 一键清除记忆 | 调用清除入口后配置目录清空、`rememberedPasswordPresent=false`；再次启动不填充密码 |
| 非法文件拒绝（5 类） | `fake-algorithm.enc`（非 AES-256 算法）、`fake-version.enc`（版本 5）、`fake-magic.enc`（magic 错误）、`short.enc`（13 B 过短）、`fake-params.enc`（customParams 超实现保护范围）全部被拒绝；提示“拒绝 5 个（不是 AES-256 加密文件、不支持的格式版本、不是 Paranoia 加密文件 等）”；列表为空时“开始解密”按钮禁用 |
| 启动耗时 | 进程启动 → 窗口创建约 0.24–0.31 s；进程启动 → 界面就绪（`ui_ready`）冷启动 740 ms，热启动 402–464 ms；3 文件 120 MB 批量（含 1 次失败）界面端到端 1.47 s |
| 打包产物运行 | `.app` 双击/`open` 启动成功，系统进程名为 `paranoia-enc-decryptor` |

**人工检查项（本环境无法自动化的部分，交付时如实说明）**：

- **物理拖拽**：已在本机用 OS 级合成事件自动化验证（见上表“物理拖拽”行：真实 `enter/over/drop` 事件流 + 文件入列），并确认拖拽落点与文件对话框共用同一 `addPaths` 处理管线（批量用例已覆盖）。真实人手拖动未试，但 OS 事件路径等价。
- **系统文件选择对话框**：实现为 Tauri dialog 插件 `blocking_pick_files`（`.enc` 过滤 + 多选），其原生弹窗点击无法自动化；多选后的处理逻辑与拖拽共用 `addPaths`，已覆盖。
- 建议用户首次运行后按 README「快速开始」做一次真实拖拽 + 对话框多选确认（1 分钟）。

---

## 7. v0.1.1 修复与重新构建（解密失败闪烁 / 新图标 / Windows 便携版）

### 7.1 问题与根因

用户反馈：批量解密时，个别文件行会**短暂显示「解密失败」，随后又变为「成功」**（最终结果正确）。

根因：core 的 `run_batch` 在 `decrypt_file` 的进度回调里携带的 `successes` / `failures` 是该文件处理**之前**的批次计数（计数在文件结束后才更新）。GUI 的 `BatchEmitter` 原先依据「`file_processed >= file_total` 且当时 `failures > 0`」推断当前文件失败并先发一条临时 `file-finished` 事件，之后真正的完成事件才把行状态改为「成功」。因此只要批次中前面有文件失败过，后续成功文件会在最后一个数据块回调时被误渲染一次「失败」。

修复（`gui/src-tauri/src/commands.rs`）：`BatchEmitter` 增加 `successes_seen` / `failures_seen` 基线；仅当进度中的成功/失败计数**相对基线发生变化**（即该文件已结算）时才发临时完成事件，且 `ok` 由计数增量判断，而非 `failures > 0`。真正完成事件仍由最终计数驱动。

### 7.2 验证（25 ms 采样 + 事件时间线）

release 二进制内置 autotest 在批处理期间每 25 ms 采样一次工具栏计数与每行状态文本，记录全部 `file-finished` 事件，并统计「最终成功但采样期间出现过『失败』文案」的次数（`transientFailOnSuccess`）。

| 场景 | `file-finished` 事件（`ok`） | 采样 | 结果 |
| --- | --- | --- | --- |
| 2 个成功 + 1 个错误密码（`bad-password.enc`：首字节损坏 → `wrongPassword`） | `true, true, false` | 整批 25 ms 采样 | `transientFailOnSuccess=0`，无任何瞬时「失败」文案；行状态最终「成功 / 成功 / 密码错误」 |
| 3 个全部成功 | `true, true, true` | 整批 25 ms 采样 | `transientFailOnSuccess=0`；成功 3 / 失败 0 |

- 两个成功输出的 SHA-256 均为 `46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840`；失败文件无输出、无临时/半成品文件残留。
- 密码记忆不受影响：新进程启动时 `autoFilled=true`、`passwordLength=3`。
- 回归：`cargo test --workspace` → 35 个 core 单元测试 + 1 个 oracle 向量测试通过，2 个 real_sample 测试 ignored；`cargo check`（GUI）通过。

### 7.3 新图标

- 母版 `gui/src-tauri/icons/app-icon.png`（1024×1024，圆角渐变底 + 白色挂锁图形）；由 `pnpm tauri icon` 生成 `.icns`（1,204,513 B）/ `.ico`（63,244 B）/ 各尺寸 PNG。
- macOS 应用图标、窗口标题栏与页面顶部品牌图标均使用新图标（`gui/src/app-icon.png` 256px，`index.html` 引用）。
- 新界面截图见 `docs/screenshots/gui-main.png`（v0.1.1）。

### 7.4 macOS arm64 重新构建（v0.1.1）

- 命令：`cd gui && pnpm tauri build`；release 增量编译 29.80 s。
- 裸二进制：4,888,448 B，SHA-256 `1af4c8d65239da5a3e49359b8ade61e37ca142243d5f33b196340ba909b6bfe2`，Mach-O arm64。
- `.app`：5.8 MiB（`du`）；`.dmg`：`Paranoia Decryptor_0.1.1_aarch64.dmg` 4,457,433 B，SHA-256 `1b4fd9ea8c25ff9fa1acf890b4147ebca46175854ff69fb8e23271072d67bb13`。
- 启动验证：运行 `.app` 内二进制，窗口正常创建；System Events 确认进程 `paranoia-enc-decryptor`；截图确认「就绪 · v0.1.1」、新图标与密码自动填充。

### 7.5 Windows x64 单文件便携版 exe

- 构建：`./scripts/build-windows-portable.sh`（内部为 `cargo xwin build --release --target x86_64-pc-windows-msvc`，rustup stable）。
- 产物：`gui/src-tauri/target/x86_64-pc-windows-msvc/release/paranoia-enc-decryptor.exe`，6,980,096 B，SHA-256 `94c617703ee413206920179af6a86ed662c8d71775e09aafabd7c387e4063805`，PE32+ GUI x86-64（Subsystem=2）。
- 单文件便携：前端静态资源与 Rust 后端全部内嵌；导入表只有 Windows 系统 DLL + UCRT（无 `VCRUNTIME140.dll`、无 `WebView2Loader.dll`），无需随附 DLL/目录。运行时依赖系统 WebView2 Runtime（Windows 11 自带；Windows 10 通常随 Edge/系统更新安装）。
- 静态 CRT 尝试失败记录与逐项检查见 `docs/WINDOWS_BUILD.md` §5.4。
- **诚实边界**：未在真实 Windows 机器/虚拟机上运行（本机无 Windows、无 Wine）；Windows 从零构建与运行验证仍以 `.github/workflows/build.yml` 的 `windows-latest` 任务为准（仓库无 remote，尚未实际执行）。

### 7.6 交付目录（桌面）

`Paranoia-Decryptor-0.1.1/`：`Paranoia-Decryptor-0.1.1-macos-arm64.dmg`、`Paranoia-Decryptor-0.1.1-windows-x64-portable.exe`、`SHA256SUMS.txt`、`使用说明.txt`。该目录是构建产物的拷贝，不属于 Git 仓库内容。

---

## 8. 构建验证

### 8.1 macOS arm64（本机从零构建并运行，v0.1.0 记录）

> 说明：以下为 v0.1.0 的从零构建与运行记录，仍然有效；v0.1.1 的重新构建与产物哈希见 §7.4。

- 环境：macOS 27.2 arm64，rustc 1.99.0，Node v26.10.0，pnpm 11.27.0，Tauri CLI 2.12.4
- 一键脚本：`./scripts/build.sh` exit 0（其内部依次执行 `cargo test --workspace`、`pnpm install`、`pnpm tauri build`）；该次 GUI 增量构建 41.37 s
- 最终 release 重新构建：`cd gui && pnpm tauri build` 成功（30.53 s，release）
- 测试：35 个 core 单元测试 + 1 个 oracle_vectors 通过，2 个 real_sample 默认 ignored
- 产物（`gui/src-tauri/target/release/`）：
  - `bundle/macos/Paranoia Decryptor.app` — 4.7 MiB；`Contents/MacOS/paranoia-enc-decryptor` 4,838,912 B
  - `bundle/dmg/Paranoia Decryptor_0.1.0_aarch64.dmg` — 2,137,598 B（`du` 2.0 MiB）
  - 裸二进制 `paranoia-enc-decryptor` — 4,838,912 B
- `file`：`Mach-O 64-bit executable arm64`（app 内二进制相同；原生 arm64，无 Rosetta）
- 部署目标：`LC_BUILD_VERSION platform MACOS, minos 11.0, sdk 27.0`
- 签名：ad-hoc / linker-signed（`flags=0x20002`），未公证；首次运行需右键“打开”或在“隐私与安全性”放行
- 实际启动：`open` 后进程与窗口正常创建（见 §6）
- CLI 释放版：750,624 B；60 MB 样本解密约 0.45 s
- **独立从零构建证明**：使用全新 `CARGO_TARGET_DIR=/tmp/paranoia-zero-target`（不触碰仓库内 `target/`）执行 `cd gui && pnpm tauri build`，全量编译 1m07s 成功；产物 `.app` 4.75 MiB、`.dmg` 2,152,698 B、裸二进制 4,855,408 B，均为 Mach-O arm64。用该全新二进制做核心验收（2 个样本副本 + 1 个首字节损坏副本，密码 `123`，未指定输出目录）：默认输出到首个文件所在目录，成功 2 / 失败 1（`wrongPassword` 隔离），两份输出 SHA-256 均为 `46abe32c…d04ce840`。

### 8.2 Windows x64

**仓库配置的权威路径**：`.github/workflows/build.yml` 含 `windows-latest` 矩阵任务，执行 `pnpm tauri build --bundles nsis` 产出 NSIS 安装包；该 workflow 是 Windows 从零构建的权威路径。**如实说明：仓库当前没有远端（no remote），workflow 尚未实际运行过**，因此本报告没有 Windows CI 运行记录。

**本机交叉编译证据**（可复现，详见 `docs/WINDOWS_BUILD.md` §4/§5）：

- 工具链：rustup stable + `cargo-xwin 0.23.1`（MSVC target `x86_64-pc-windows-msvc`）；必须使用 rustup 的 stable 工具链（Homebrew rustc 缺 Windows std，报 E0463）
- 产物：
  - `paranoia-enc-cli.exe` — 781,312 B
  - `paranoia-enc-decryptor.exe`（GUI）— v0.1.0 首次为 6,886,912 B；v0.1.1 重新交叉编译后为 6,980,096 B 的**单文件便携版**（见 §7.5）
  - 均为 PE32+ x86-64；GUI 子系统 `Subsystem=2`（Windows GUI）
- **未在 Windows 上执行**，也未用 Wine 模拟运行冒充原生验证（本机未安装 Wine）；无 NSIS 安装包（交叉编译不产出 NSIS）。
- 结论：Windows x64 的“从零构建并成功运行”证据分为两段——交叉编译证明代码可面向 MSVC x64 构建；原生运行需推送仓库到 GitHub 后由上述 workflow 产出并安装验证。

---

## 9. 复现命令

```bash
# 1) 单元/集成测试
cargo test --workspace

# 2) CLI（本机）
cargo run --release -p paranoia-enc-cli -- --inspect <sample>.enc
cargo run --release -p paranoia-enc-cli -- <sample>.enc -p <password>

# 3) 真实样本（需本地样本）
PARANOIA_SAMPLE_ENC=... PARANOIA_SAMPLE_SHA256=... PARANOIA_SAMPLE_PASSWORD=... \
  cargo test -p paranoia-enc-core --test real_sample -- --ignored --nocapture

# 4) 独立 Java Oracle
bash verification/oracle/test.sh

# 5) 一键构建 GUI + 测试
./scripts/build.sh
```

---

## 10. 已知限制（与首期范围一致）

- 仅支持 Paranoia v4 容器、AES-256（algorithm code 0）；其他算法/版本明确报错。
- 不支持 PFTEREC1 内嵌恢复数据（检测到时明确报 `unsupportedRecovery`，不猜测）。
- 不实现加密。
- 密码记忆为本地明文存储（路径与风险见 README）；不联网、不上传。
- Windows x64：本机无 Windows 物理机/虚拟机，已完成 cargo-xwin 交叉编译（CLI exe 与 GUI 单文件便携版 exe，均为 PE32+；便携版见 §7.5）；原生运行验证依赖仓库推送后 `.github/workflows/build.yml` 的 windows-latest 任务，报告中如实区分。
