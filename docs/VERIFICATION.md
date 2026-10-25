# 验证（VERIFICATION）— v0.3.2

> 结论先行：**本机可复现的自动测试 `cargo test --workspace` 114 通过 / 0 失败 / 2 ignored，
> CLI e2e 104 项全绿（ALL E2E CHECKS PASSED）**。真实 Paranoia 样本的
> MAC/checkcode/CRC/明文一致性属于「历史外部验证」——2026-10-23 用独立 Java Oracle
> 跑过并通过，但当前机器无法重跑（缺少 Oracle 前置与原始明文）。v0.3.1 的**分卷压缩包**
> 在合成 fixture（e2e）与**真实样本 3138.7z.001/002.enc（Lead 手工复验）**上均验证；
> v0.3.2 的**跳过已处理默认关闭 / 分卷行归并（`related_paths`）/ Windows 无控制台窗口 /
> 日志落盘与诊断信息**已在本机（macOS 运行 + 单测 + e2e）验证；Windows GUI 单 exe 仍只做了
> 交叉编译与静态证据，**未在真实 Windows 上启动**。
> 本文严格区分「本机已验证」「历史记录」「断言但未验证」，不把三者混为一谈。

## 1. 环境

| 项 | 值 |
|---|---|
| 主机 | macOS 27.2 arm64 |
| Rust | rustc 1.99.0 / cargo 1.99.0 |
| Node / pnpm | v26.10.0 / 11.27.0 |
| 7-Zip | 随仓库 `binaries/mac/7zz` 25.01（2025-08-03），SHA-256 见 binaries/README.md |
| Windows 交叉编译（历史） | rustup target `x86_64-pc-windows-msvc` + cargo-xwin 0.23.1（首次下载约 1.1 GB MSVC CRT/SDK） |
| Java Oracle（历史） | Temurin JDK 21.0.13+9，官方 Paranoia 源码树位于 `/tmp/paranoia-pc`（**当前不存在**） |

## 2. 自动测试（本机可复现）

命令：

```bash
cargo test --workspace
```

2026-10-24 v0.3.2 最终构建前实测（exit 0；重建后再跑以实际输出为准）：

| target | 通过 | 失败 | ignored | 内容 |
|---|---|---|---|---|
| `unparanoia_core`（unit） | 43 | 0 | 0 | decrypt 15、format 7、config 6、output 5、kdf 4、crypto 3、batch 3 |
| `tests/oracle_vectors.rs` | 1 | 0 | 0 | 回放 `verification/oracle/test-vectors.json`（见 §3） |
| `unparanoia_embedded_7z`（unit） | 3 | 0 | 0 | `license_text_is_embedded`、`non_windows_is_a_noop`、`version_is_reported` |
| `unparanoia_unpack_core`（unit） | 65 | 0 | 0 | detect 9、engine 14（分卷/逐层多密码/跳过记录/tokio 用例）、split 7、layout 7、settings 7、sevenzip 7、ssefe 5、processed 3、sfx 3、queue 3 |
| `unparanoia_lib`（GUI unit） | 2 | 0 | 0 | `civil_from_days_matches_known_epoch_days`、`shorten_only_appends_ellipsis_when_cut` |
| `tests/real_sample.rs` | 0 | 0 | **2** | 真实样本解密（需环境变量，见 §5） |
| CLI / GUI main / doc-tests | 0 | 0 | 0 | 无测试 |
| **合计** | **114** | **0** | **2** | 2026-10-24 v0.3.2 实测；耗时约 3.5 s（release 构建另计） |

`oracle_vectors` 的比对范围（`crates/core/tests/oracle_vectors.rs:82-103`）：
header raw/version/algorithm/customParams、salt、Argon2 t/m/p/output_len、
`derive_l0_password_hash`、`encKey/nonce/authKey` 的字节相等。
**它不比对 MAC、checkcode、ZIP CRC、明文 SHA-256**——那些只由外部 Java Oracle
在 2026-10-23 验证过（§4），不在 Rust 测试的回放范围内。任何声称
「Rust 测试验证了 MAC」的说法都是不准确的。

## 3. Oracle 测试向量（`verification/oracle/test-vectors.json`）

来源：独立 Java Oracle 对两个真实样本 `3138.7z.001.enc` / `3138.7z.002.enc`
（密码 `123`，customParams `0x11`）的输出快照，提交进仓库供离线回归。
每个 sample 字段：`headerHex`、`customParamsByte`、`argon2Params`、`saltHex`、
`l0PWHashV3Hex`（128 B）、`argon2OutputHex`（256 B）、`encKeyHex`/`nonceHex`/
`authKeyHex`、`storedMacHex`/`computedMacHex`/`macMatch`、`checkCode*`、
`innerZipSize`/`zipEntries[]`（CRC/SHA-256）、`decryptedSha256`/`expectedSha256`、
`byteCompareWithOriginal` 等。另有 `negativeTests`（错误口令）。

测试只断言 Rust 实现与向量中的 **KDF/密钥派生** 一致；向量文件里 MAC/CRC/
明文字段是 Oracle 的历史输出记录，不是 Rust 测试的断言对象。

## 4. 真实样本的外部 Java Oracle 验证（历史记录，当前不可重跑）

- 文档：`docs/oracle-report.md`（362 行，含逐命令/逐字节记录）；
- 实现：`verification/oracle/src/oracle/Oracle.java`（690 行，独立重写容器解析与
  KDF 接线，密码学原语复用官方 Java 源码树，Rust 实现**未被参考**）；
- 样本：`/Volumes/Carrot/Download/3138.7z.001.enc`（209,779,415 B）与
  `3138.7z.002.enc`（60,527,110 B），原明文 `.7z` 文件当时在样本旁；
- 结论（2026-10-23）：两个样本 MAC 通过、checkcode 为 32 位 `[a-z0-9]` 合法、
  内层 ZIP 每个条目 CRC32 与 SHA-256 通过、解密明文 SHA-256 与期望值一致、
  `cmp` 与原始文件逐字节一致；错误口令 `456` 被拒绝。

期望明文 SHA-256（记录于 oracle-report 与 `verification/oracle/test.sh`）：

```text
001: 9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c
002: 46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840
```

**当前不可重跑的原因**（2026-10-24 核查）：`/tmp/jdk21` 与 `/tmp/paranoia-pc`
均不存在；`/Volumes/Carrot/Download` 只剩两个 `.enc`，原明文 `.7z` 已不在，
`cmp` 无法重做。仓库内的 `test-vectors.json` 是唯一可离线回放的部分。

复现前提（未来要重跑时）：Temurin JDK 21 + 官方 Paranoia v4 源码树
（`PARANOIA_PC` 指向含 `CryptoLibs/src` 的目录）+ 原始 `.7z` 明文文件。
然后：

```bash
./verification/oracle/build.sh
./verification/oracle/test.sh          # 最后一行: ALL CHECKS PASSED (MAC, checkcode, CRC, SHA-256, cmp, wrong-password)
```

## 5. Rust 真实样本测试（默认 ignored，需外部样本）

`crates/core/tests/real_sample.rs` 两个测试：

```bash
PARANOIA_SAMPLE_ENC=/path/3138.7z.001.enc \
PARANOIA_SAMPLE_SHA256=9d001d6c...e40c \
cargo test -p unparanoia-core --test real_sample -- --ignored
```

- 测试 1：`decrypt_file` 输出与期望 SHA-256 一致；
- 测试 2：batch 对同一输入的两个副本各解一次。
- 注意：`PARANOIA_SAMPLE_ENC` 是**单个**文件（测试 2 会复制成两份），
  不是目录。

## 6. CLI 端到端（e2e，本机可复现）

```bash
./scripts/make-fixtures.sh     # 生成 testdata/fixtures/（需要 binaries/mac/7zz；结构可复现，非字节稳定）
./scripts/e2e-verify.sh        # 2026-10-24 v0.3.2: 104 项检查全部通过，最后一行 ALL E2E CHECKS PASSED
```

覆盖（`scripts/e2e-verify.sh`，独立临时配置目录，不碰真实配置）：

1. `--version` 含 0.3.2；`doctor` 报告 7-Zip；`--help` 列出子命令；
   `--licenses` 输出内嵌/随包 7-Zip 许可证（含 unRAR 限制文字）；
2. `detect --json` 识别 `ssefe_encrypted` / `masked_archive`；`inspect` 显示 SSEFE 头；
3. `sample.enc`（SSEFE，pw 123）解密出 `hello.txt`（fixture 内容为
   `hello unparanoia v0.3.1`），SHA-256 匹配，无 staging 残留；
4. `nested.enc` → `nested.zip` → `nested-inner.zip` → `hello.txt` 级联；
5. `normal.zip` smart layout 产出 `normal/hello.txt` + `normal/docs/readme.txt`；
   加密 7z（`-mhe=on`，pw 123）与前缀 ZIP（`masked.mp4`，4 KiB 前缀）均正确解出；
6. 错误口令 / 无口令：exit 1、提示需要密码、零产出；
6b. **逐层多密码**：`layered.enc`（外层 pw=outer，内层 7z pw=inner）用
   `-p wrong -p outer -p inner` 一次成功，stderr 出现 `使用候选密码` 警告；
   只给外层密码时 **exit 0（部分成功）**：保留已解出的内层归档
   `encrypted-inner.7z`、不产生 `hello.txt`，stderr 给出
   「内层归档「…」需要密码…保留外层内容」警告。
   **这是 v0.3 的既定语义，不记为失败**：若把内层缺密码记为 NeedsPassword/Failed，
   重试会重做外层并产生重复输出；保留内层归档可让用户单独补密码再解。
   这也满足「失败不得破坏已成功输出」的约束（外层成果留在输出目录）；
7. `--json` 输出 JSON Lines：`task_detected`、`task_finished`、末行 `{"type":"summary",...}`，
   每行可被 `json.loads` 解析；
8. `--dry-run` 不写文件；`--conflict rename` 生成 `hello (1).txt`；`--layout folder` → `sample/hello.txt`；
9. `config --show/--set-password/--clear-password`（v0.2.0 兼容）以及
   `--add-password/--list-passwords/--remove-password/--clear-passwords`：
   密码库文件创建、Unix 0600、无 `-p` 时自动用密码库解密成功、删空/清空；
   `cleanup-staging` 只删 `.unparanoia-tmp-*`；
10. **分卷压缩包（v0.3.1）**：
    (a) 普通分卷 `split.7z.001`（同目录有 002-004）→ exit 0，输出
        `split/hello.txt` 与 `split/blob.bin`，SHA-256 均匹配；
    (b) 只给非首卷 `split.7z.002.enc`（隔离目录、无 001）→ **exit 0（Skipped）**，
        输出含「第一卷」提示，且零产出（不失败、不产半成品）；
    (c) SSEFE 分卷组 `split.7z.001.enc` + `split.7z.002.enc`（每卷是独立
        Paranoia v4 容器；同族其余卷自动发现）用 `-p wrong -p 123` → exit 0，
        JSON 断言：**合并为 1 个任务、summary total=1/success=1、files=2、
        container=7z-volume、warnings 含「使用候选密码」**，输出 SHA-256 匹配；
    (d) 缺后续卷 `truncated.7z.001.enc`（只有第 1 卷）→ **exit 1**，报
        「分卷压缩包缺少后续卷」，零半成品；
11. 用法错误（非法 `--conflict` 值、未知参数）exit 2；
12. **跳过已处理默认关闭（v0.3.2）**：同一 `sample.enc` 在隔离配置目录下默认连续跑两次
    都 exit 0 且两次 summary 均 `success=1/skipped=0`，**不生成 `processed.json`**；
    显式 `--skip-processed` 时第一次写 `processed.json`、第二次 exit 0 但 outcome
    `status=skipped`、`reason` 含「已处理过」、summary `skipped=1/success=0`；
    删除 `processed.json` 后再默认运行恢复 success。另断言分卷 SSEFE `--json` 的
    `TaskOutcome.related_paths` 含该族全部 4 个 `.enc` 卷（行归并依据，全部存在且
    以 `.enc` 结尾）。

fixture 内容（`testdata/fixtures/`，gitignored；每次生成 mtime/salt 不同，但内容与结构一致）：
`normal.zip`、`inner.zip`、`nested.zip`、`encrypted.7z`（pw 123）、
`encrypted-inner.7z`（pw inner）、`masked.mp4`、
`sample.enc`（make_enc 生成，pw 123，customParams 0x00）、`nested.enc`、
`layered.enc`（外层 pw outer、内层 7z pw inner）、
`split.7z.001..004`（普通分卷，1 MiB 一卷，含 3 MiB 零填充 blob）、
`split.7z.001.enc..004.enc`（每卷分别被 Paranoia v4 加密，SSEFE 分卷组）、
`truncated.7z.001.enc`（只有首卷，用于缺卷失败用例）、`SHA256SUMS`。

## 7. 构建产物（v0.3.2 最终记录：2026-10-24）

> **状态：由 Lead 在代码冻结后执行最终构建。** macOS `./scripts/build.sh` exit 0
> （其中 `cargo test --workspace` = 114 passed / 0 failed / 2 ignored）；
> Windows 便携版 `./scripts/build-windows-portable.sh` exit 0
> （cargo-xwin，仅有 LNK4099 PDB 警告，与产物无关）。产物路径均在 workspace 根 `target/`。
> 每次重建（即使源码相同）哈希都会变；重建后必须更新本表。本表数值在构建后由
> docs-reader 在磁盘上逐项复核（大小 + SHA-256 + DMG 挂载 + zip 内容列表）。

| 产物 | 大小 (B) | SHA-256 |
|---|---|---|
| `target/release/bundle/dmg/Unparanoia_0.3.2_aarch64.dmg` | 8,248,449 | `1f5294ca65bdde5c369a8e7d77bf44c67d626e06ab49b15b73e35c9cf5254db3` |
| `.app` 内主二进制（`bundle/macos/Unparanoia.app/Contents/MacOS/unparanoia`） | 7,431,152 | `d147d2f5200b48d9fa0efb25205f242d23edff860efc1d8100e74c4db0a74da5` |
| GUI 裸二进制 `target/release/unparanoia` | 7,431,152 | `d147d2f5…`（与 .app 内相同） |
| CLI `target/release/unparanoia-cli` | 2,789,168 | `6e25b1a5c32607ef2a8d6038c6cde3dd569e58865fe36470a108af28a5101759` |
| Windows 便携 zip `target/windows-portable/Unparanoia-0.3.2-win-x64-portable.zip` | 7,160,723 | `465490d7311a1aad4328baf825d0807f9d62d6559e54ff13274c1ae457cc2254` |
| zip 内 GUI `Unparanoia.exe`（PE32+ GUI x86-64） | 13,758,464 | `e1c18e51e39aacb186a945ae8efcbbbd7c7008ba9224a32c166feba283fbdf99` |
| zip 内 CLI `unparanoia-cli.exe`（PE32+ console x86-64） | 6,211,584 | `ee54556a1c24a79dadedcd25496174e627aa0f0c2fafd953d3332920551c2fa1` |
| zip 内 `7z-LICENSE.txt` | 6,029 | 与 `binaries/7z-LICENSE.txt` 一致（`99df5a51…`） |

当次核对（Lead 构建 + docs-reader 磁盘复核）：

- DMG 以 `hdiutil attach -readonly` 挂载成功，含 `Applications` 链接与
  `Unparanoia.app`（`CFBundleShortVersionString`=0.3.2、
  `CFBundleIdentifier`=com.nanase.unparanoia、minOS 12.0）；`.app` 主二进制与
  裸二进制 SHA-256 一致；`Contents/Resources/binaries/mac/7zz` 存在且哈希与
  `binaries/mac/7zz` 一致（`5c2fd36f…`）；detach 正常。`unparanoia-cli --version`
  输出 `unparanoia-cli 0.3.2`。
- Windows 便携 zip 用 `7zz l` 核对为 **5 个文件 + 1 个顶层目录**
  `Unparanoia-0.3.2-win-x64-portable/`（`Unparanoia.exe`、`unparanoia-cli.exe`、
  `7z-LICENSE.txt`、`README.txt`、`SHA256SUMS`；**无** 7z.exe/7z.dll 侧车）；
  `SHA256SUMS` 与上表一致（Lead 解压后 `shasum -c` 全部 OK）。
- `codesign --verify --deep --strict` 对 .app **仍失败**（"code has no resources but
  signature indicates they must be present"），即产物为 ad-hoc 签名、不可直接分发；
  分发前必须用 Developer ID 重签并公证。

**GUI 启动冒烟（人工，2026-10-24 v0.3.2 最终构建后由 Lead 执行，通过）**：
`open ./target/release/bundle/macos/Unparanoia.app` 进程存活；`CGWindowList`
查到窗口 owner=`Unparanoia`、title=「Unparanoia 解包器 — 批量解密/解压」、
尺寸 1120x780。启动后 GUI 日志实际落盘：
`~/Library/Application Support/unparanoia/logs/unparanoia.log`（启动即写 4 行，
含 7-Zip 路径/版本/来源）。解锁桌面并激活窗口后已抓取实际界面截图
`docs/screenshots/gui-0.3.2.png`（2240×1560 px，1120×780 pt @2x，截自最终产物）：
可见顶栏「Unparanoia」+ 版本 0.3.2、内嵌 7-Zip 25.01 横幅、配置文件路径、
队列/日志/密码/设置四个 tab 与空队列提示；界面与 v0.3.0 布局相同（v0.3.1 新增
设置页「7-Zip 许可证」入口，v0.3.2 新增设置页「诊断信息」与日志入口）。
边界：仅启动级冒烟；交互式切页/拖放/暂停取消未做（尝试 System Events 自动化时
accessibility 被系统拒绝，错误 -25208），**不等于**完整 GUI 交互测试（见 §10）。

## 8. 分卷压缩包与跳过行为验证（v0.3.1/v0.3.2）

实现：`crates/unpack-core/src/split.rs`（卷名解析/同族聚合/缺卷检测）+
`crates/unpack-core/src/engine.rs`（聚合为一个逻辑任务；SSEFE 分卷逐卷解密后按
7-Zip 分卷读取；普通分卷直接交给 7-Zip；缺卷/缺密码均明确报错且不留半成品）。

**合成 fixture（§6 第 10 项，本机可复现）**：普通分卷、SSEFE `.enc` 分卷组、
只给非首卷（Skipped + 提示第一卷）、缺后续卷（Failed + 零半成品）全部通过。

**真实样本（Lead 手工复验，2026-10-24 18:02 与二次复验）**：
release CLI，样本 `/Volumes/Carrot/Download/3138.7z.001.enc`（209,779,415 B）+
`3138.7z.002.enc`（60,527,110 B），Paranoia 口令 `123`、内层 7z 口令 `anzixin`：

| 场景 | 结果 |
|---|---|
| A. 两个 `.enc` 一起（`-p 123 -p anzixin`） | 1 个任务 **Success**；`TaskDetected`=archive/container `7z-volume`/format `7z`/entries 6/`top_level`=["3138"]；输出 `3138/` 目录 **5 个文件、369,993,992 B（352.85 MiB）**；耗时 1.6 s；warnings 含「使用候选密码 #2 成功」 |
| B. 只给 `3138.7z.001.enc`（同目录有 `.002.enc`） | 自动发现其余卷并成功 |
| C. 只给 `.002.enc`（无 `.001`） | **Skipped（exit 0）**，提示「分卷压缩包：请添加第一卷（.001 / .part1.rar）；期望首卷：3138.7z.001.enc」 |
| D. 只给 `.001.enc` 且缺 `.002` | **Failed（整批 exit 1）**，报「分卷压缩包缺少后续卷（找到 1/2 卷）」 |

注：真实样本不在仓库内（`/Volumes/Carrot/Download`，外部介质），A-D 无法在本仓库
CI/单测中重放；合成 fixture 覆盖同一批分支。

**v0.3.2 默认行为回归（Lead 手工复验，release CLI，默认配置目录）**：

- 两个 `.enc` 一起（默认配置）→ 1 任务 Success，**2.2 s**，输出 5 文件 **353 MB**；
- **默认第二次运行仍 Success**（v0.3.2 起跳过已处理默认关闭，不生成 `processed.json`）；
- 显式 `--skip-processed` 时第二次运行 **Skipped 2** 并生成 `processed.json`
  （reason 含「已处理过」）；
- 只给 `.001` 自动发现同目录 `.002` 并成功；只给 `.002` 且无 `.001` → Skipped
  提示「请添加第一卷」；只给 `.001` 且缺 `.002` → Failed「缺少后续卷（找到 1/2 卷）」；
- `unparanoia-cli doctor` 正常。

v0.3.2 的跳过语义细节：`Settings.settingsVersion` 默认 2；读取旧 `settings.json`
（无该字段/`<2`）时一次性迁移为 `skip_processed=false` 并写回 2（单测
`legacy_files_without_version_migrate_skip_processed`）。队列行合并依据
`TaskOutcome.related_paths`（分卷族全部卷路径），见 `docs/OUTPUT-LAYOUT.md` §2.1。`

## 9. Windows 单 exe、白屏修复、无控制台窗口与诊断日志（v0.3.1/v0.3.2）

**白屏根因**：Windows GUI 若只做裸 `cargo build`，缺少 `--features custom-protocol`，
`tauri` 的 codegen 会按 dev 模式处理、不把 `frontendDist` 嵌进 exe，运行时去加载
`http://localhost:1420`，用户机器上自然 `ERR_CONNECTION_REFUSED`/白屏。
修复：`gui/src-tauri/Cargo.toml` 增加 `[features] custom-protocol = ["tauri/custom-protocol"]`，
所有 Windows GUI 构建路径（`scripts/build-windows-portable.sh`、
`scripts/build-windows.ps1`、CI Windows job、`tauri build` 本身）都必须带该 feature；
CLI crate 没有此 feature，两条 `-p` 命令必须分开。

**v0.3.1 验收证据（Lead 构建时核对）**：

- 单 exe 生效：GUI exe 内 `tauri://localhost` 标记命中，说明 custom-protocol 已起效
  （不再是 devUrl `http://localhost:1420`）；
- 内嵌 7-Zip：两个 exe 均**逐字节内嵌** `binaries/win/7z.exe`（575,488 B）、
  `7z.dll`（1,906,176 B）、`7z-LICENSE.txt`（6,029 B）——用 Python 字节包含检测
  `embedded=True`；包内**不再有**侧车 `7z.exe`/`7z.dll`；
- 首次运行把内嵌运行时解压到 `%LOCALAPPDATA%\Unparanoia\embedded-7z\25.01-r1\`
  （可用 `UNPARANOIA_7Z_CACHE_DIR` 覆盖）；GUI 找不到内嵌/外部 7-Zip 时不阻止启动，
  CLI `doctor` 显示 7-Zip 来源；
- `unparanoia-cli --licenses` 可查看内嵌/随包 7-Zip 许可证。

**v0.3.2 Windows 相关修复（本机静态/单测验证，真机未跑）**：

- **无控制台闪窗**：`crates/unpack-core/src/sevenzip.rs` 在 Windows 上以
  `CREATE_NO_WINDOW`（`0x0800_0000`）spawn `7z.exe`（`run()` 与 `version()` 均设置），
  GUI 子系统进程不再弹出黑色控制台窗口；
- **可操作错误信息**：7-Zip 失败信息包含 7z 路径、OS 错误/退出码、stderr 尾部
  （≤1000 字符）以及「可设置 `UNPARANOIA_7Z` 指向已安装 7-Zip」的提示
  （单测 `failure_messages_are_actionable_and_bounded`）；
- **spawn 重试**：Windows 锁文件错误码 5/32/225 最多重试 3 次、间隔 150 ms
  （单测 `retryable_spawn_errors_are_windows_lock_codes`）；
- **日志与诊断（跨平台，macOS 已实测）**：日志 `<configDir>/logs/unparanoia.log`
  （>2 MiB 轮转到 `unparanoia.log.1`，只保留 1 份），GUI 设置页「诊断信息」提供
  复制诊断 JSON / 打开日志目录 / 清除处理记录；`AppInfo` 增加 `logPath`/`logDir`；
  启动后日志实际生成（见 §7 GUI 冒烟）。

**诚实边界**：以上均为 macOS 主机交叉编译产物上的静态证据（v0.3.2 另加单测与
macOS 日志实测）；**未在真实 Windows 11 上启动 GUI/CLI**，未验证 WebView2、中文路径、
首次解压缓存写入权限、控制台窗口是否真的不出现、SmartScreen（未做 Authenticode 签名）。
详见 `docs/WINDOWS_BUILD.md` §5。

## 10. 断言但未验证 / 不可复现清单（必须诚实保留）

| 项 | 状态 |
|---|---|
| 真实样本 MAC/checkcode/CRC/明文一致性 | 历史 Java Oracle 验证通过；**当前不可重跑**（缺 /tmp/paranoia-pc、/tmp/jdk21、原明文） |
| `test-vectors.json` 的 MAC/CRC 字段 | 是历史记录，Rust 测试不断言它们 |
| real_sample.rs 两个测试 | 默认 ignored；本机无样本，未执行 |
| 真实样本分卷（3138.7z.001/002.enc） | Lead 手工复验通过（§8），但样本不在仓库、本机无法重跑；仓库内只有合成 fixture 的回归 |
| GUI 手工交互（拖放、暂停/继续/取消、重试、密码管理页、主题切换、7-Zip 许可证弹窗） | 未做 GUI 自动化测试；仅保证 `pnpm build`、tauri 打包通过；最终构建启动冒烟通过（标题/版本/窗口尺寸），交互切页/拖放等未验证（System Events accessibility 被拒 -25208） |
| 密码库安全性（明文落盘） | 设计即明文 `config.json`（Unix 0600，Windows 无权限位）；这是有意的便利取舍，不是加密保险箱 |
| 逐层多密码重试的 GUI 路径 | CLI e2e 已覆盖（`layered.enc`）；GUI 只保证命令/界面存在，未做交互自动化 |
| 跳过已处理默认关闭与旧配置迁移 | 单测覆盖 `settingsVersion`<2 → `skip_processed=false` 并写回 2；e2e 覆盖默认连续两次 success / `--skip-processed` 第二次 skipped；未在真实用户的旧 `settings.json` 上单独演练（Lead 已清理真实 `processed.json` 中的调试记录） |
| 日志轮转（>2 MiB → `.1`） | 代码实现并有 GUI 日志落盘实测，但未做触发轮转的容量实测 |
| 分卷队列行归并（`related_paths`）的 GUI 呈现 | CLI e2e 已断言 `related_paths` 4 卷；GUI 行归并/全跳过提示条未做交互自动化 |
| Windows 真实运行、WebView2、中文路径、单 exe 首次解压缓存、无控制台窗口 | 未验证（用户将在真机验证）；v0.3.2 的 CREATE_NO_WINDOW / spawn 重试 / 错误细节仅有单测与静态证据 |
| Windows NSIS 安装包 | 路线存在（`scripts/build-windows.ps1`）但未构建、未验证；正式交付走单 exe 便携版 |
| DMG 安装体验/签名/公证 | 未签名、未公证；codesign 校验失败 |
| CI（.github/workflows/build.yml） | 仓库无远端，workflow 从未在 GitHub 上跑过；本地仅手工执行同等命令 |
| 7-Zip 二进制与官方下载的逐字节来源比对 | 未做（无网络）；只能靠版本字符串 + 记录哈希 + 继承来源 |
| 自解压 EXE 执行回退（`--allow-execute-sfx`） | 默认关闭，Windows-only，未验证 |
| PFTEREC1 恢复容器 | 明确不支持（v3 与恢复容器均未实现） |

## 11. 复现全部本机可复现项

```bash
cargo test --workspace
./scripts/make-fixtures.sh
./scripts/e2e-verify.sh
./scripts/build.sh                # macOS 最终构建（dmg + .app + CLI）
./scripts/build-windows-portable.sh   # 可选：Windows 单 exe 交叉编译
```
