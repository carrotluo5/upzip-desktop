# 验证（VERIFICATION）

> 结论先行：**本机可复现的自动测试（cargo test --workspace）+ CLI e2e（75 项）全绿；
> 真实 Paranoia 样本的 MAC/checkcode/CRC/明文一致性属于"历史外部验证"**——
> 它在 2026-10-23 用独立 Java Oracle 跑过并通过，但当前机器无法重跑
> （缺少 Oracle 前置与原始明文文件）。本文严格区分「本机已验证」「历史记录」
> 和「断言但未验证」，不把三者混为一谈。v0.3.0 新增的密码库/逐层重试由
> CLI e2e 与本机单测覆盖；主题与密码管理页的 GUI 交互仍属"仅人工冒烟"。

## 1. 环境

| 项 | 值 |
|---|---|
| 主机 | macOS 27.2 arm64 |
| Rust | rustc 1.99.0 / cargo 1.99.0 |
| Node / pnpm | v26.10.0 / 11.27.0 |
| 7-Zip | 随仓库 `binaries/mac/7zz` 25.01（2025-08-03），SHA-256 见 binaries/README.md |
| Java Oracle（历史） | Temurin JDK 21.0.13+9，官方 Paranoia 源码树位于 `/tmp/paranoia-pc`（**当前不存在**） |

## 2. 自动测试（本机可复现）

命令：

```bash
cargo test --workspace
```

2026-10-24 v0.3.0 实测结果（`cargo test --workspace`，exit 0；下表为最后一次记录，
若本轮改动后重跑请以实际输出为准）：

| target | 通过 | 失败 | ignored | 内容 |
|---|---|---|---|---|
| `unparanoia_core`（unit） | 43 | 0 | 0 | decrypt 15、format 7、config 6、output 5、kdf 4、crypto 3、batch 3 |
| `tests/oracle_vectors.rs` | 1 | 0 | 0 | 回放 `verification/oracle/test-vectors.json`（见 §3） |
| `unparanoia_unpack_core`（unit） | 47 | 0 | 0 | detect 9、engine 8（含 password_candidates 顺序/去重/重试）、settings 6、layout 6、ssefe 5、sevenzip 5、sfx 3、processed 3、queue 2 |
| `tests/real_sample.rs` | 0 | 0 | **2** | 真实样本解密（需环境变量，见 §5） |
| CLI / GUI / doc-tests | 0 | 0 | 0 | 无测试 |
| **合计** | **91** | **0** | **2** | 2026-10-24 v0.3.0 实测；耗时约 3.3 s（release 构建另计） |

`oracle_vectors` 的比对范围（`crates/core/tests/oracle_vectors.rs:82-103`）：
header raw/version/algorithm/customParams、salt、Argon2 t/m/p/output_len、
`derive_l0_password_hash`、`encKey/nonce/authKey` 的字节相等。
**它不比对 MAC、checkcode、ZIP CRC、明文 SHA-256**——那些只由外部 Java Oracle
在 2026-10-23 验证过（§4），不在 Rust 测试的回放范围内。任何声称
"Rust 测试验证了 MAC"的说法都是不准确的。

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

复现前提（未来要重跑时）： Temurin JDK 21 + 官方 Paranoia v4 源码树
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
./scripts/e2e-verify.sh        # 2026-10-24 v0.3.0: 75 项检查全部通过，最后一行 ALL E2E CHECKS PASSED
```

覆盖（`scripts/e2e-verify.sh`，独立临时配置目录，不碰真实配置）：

1. `--version` 含 0.3.0；`doctor` 报告 7-Zip；`--help` 列出子命令；
2. `detect --json` 识别 `ssefe_encrypted` / `masked_archive`；`inspect` 显示 SSEFE 头；
3. `sample.enc`（SSEFE，pw 123）解密出 `hello.txt`（v0.3.0 fixture 内容为
   `hello unparanoia v0.3.0`），SHA-256 匹配，无 staging 残留；
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
   这也满足"失败不得破坏已成功输出"的约束（外层成果留在输出目录）；
7. `--json` 输出 JSON Lines：`task_detected`、`task_finished`、末行 `{"type":"summary",...}`，
   每行可被 `json.loads` 解析；
8. `--dry-run` 不写文件；`--conflict rename` 生成 `hello (1).txt`；`--layout folder` → `sample/hello.txt`；
9. `config --show/--set-password/--clear-password`（v0.2.0 兼容）以及
   `--add-password/--list-passwords/--remove-password/--clear-passwords`：
   密码库文件创建、Unix 0600、无 `-p` 时自动用密码库解密成功、删空/清空；
   `cleanup-staging` 只删 `.unparanoia-tmp-*`；
10. 用法错误（非法 `--conflict` 值、未知参数）exit 2。

fixture 内容（`testdata/fixtures/`，gitignored；每次生成 mtime/salt 不同，但内容与结构一致）：
`normal.zip`、`inner.zip`、`nested.zip`、`encrypted.7z`（pw 123）、
`encrypted-inner.7z`（pw inner）、`masked.mp4`、
`sample.enc`（make_enc 生成，pw 123，customParams 0x00）、`nested.enc`、
`layered.enc`（外层 pw outer、内层 7z pw inner）、`SHA256SUMS`。

## 7. 构建产物（v0.3.0，macOS arm64）

> **状态：已在 2026-10-24 16:13（macOS）与 16:16（Windows 交叉编译）由 Lead
> 在代码冻结后构建。** 下表是**当次**产物的实测值；每次重建（即使源码相同）哈希都会变，
> 重新构建后必须更新本表。产物路径均在 workspace 根 `target/release/`。
>
> 与旧版（v0.2.0）的关系：v0.2.0 时代的产物数字已过时，且代码/名称均已变化，
> 本文件不再保留旧哈希，避免误引用。

| 产物 | 大小 (B) | SHA-256 |
|---|---|---|
| `target/release/bundle/dmg/Unparanoia_0.3.0_aarch64.dmg` | 8,185,594 | `721c862545914cd11c707f3a2ce554100f2b3822b7e369d1a91816bb2f5b6e1a` |
| `.app` 内主二进制（`bundle/macos/Unparanoia.app/Contents/MacOS/unparanoia`） | 7,315,392 | `76d9c9018d532b66a3c2ba0d4e5bb7ceffadf398f4e9c765ee3fdc2367b69ddb` |
| GUI 裸二进制 `target/release/unparanoia` | 7,315,392 | `76d9c9018d532b66a3c2ba0d4e5bb7ceffadf398f4e9c765ee3fdc2367b69ddb`（与 .app 内相同） |
| CLI `target/release/unparanoia-cli` | 2,706,464 | `75730f72f9122abba716c329d5ef45c86abd0815d6d2ccd9e2741fd9e2c13d74` |
| Windows GUI exe（xwin，`target/x86_64-pc-windows-msvc/release/unparanoia.exe`） | 10,962,944 | `15df510b5f8ec458748fe2cdb9ac97e7affa516ee69f7dc0c33e2c35918f8086` |
| Windows CLI exe（xwin，`target/x86_64-pc-windows-msvc/release/unparanoia-cli.exe`） | 3,584,000 | `34ceb172e448a00f2902046cc57f49a5dc140f43806e3267a335b4c224de0a50` |
| Windows 便携 zip `Unparanoia-0.3.0-win-x64-portable.zip` | 5,855,710 | `8799f0476b5d5247f089071fcbd5a1d75c51f2d8260149c3738e8c06b99df187` |

当次核对（2026-10-24 16:20）：

- 两个 Mach-O 均为 arm64；DMG 挂载后 `.app` 主二进制与裸二进制 SHA-256 一致，
  `Contents/Resources/binaries/mac/7zz` SHA-256 为 `5c2fd36f…`（与 `binaries/mac/7zz` 一致），
  `Info.plist` 为 0.3.0 / `com.nanase.unparanoia` / minOS 12.0。
- Windows 两个 exe 为 PE32+ x86-64（GUI / console）；便携 zip 用 `7zz l` 核对为 7 个文件、
  1 个目录，zip 内 `SHA256SUMS` 与各文件（含 7z.exe/7z.dll/7z-LICENSE.txt）自洽。
- `codesign --verify --deep --strict` 对 .app **仍失败**（"code has no resources but
  signature indicates they must be present"），即产物为 ad-hoc 签名、不可直接分发。

构建命令：macOS 用 `./scripts/build.sh`（先 `cargo test --workspace`，再
`pnpm --dir gui install --frozen-lockfile` + `pnpm --dir gui tauri build`，最后构建 CLI）；
Windows 便携版用 `./scripts/build-windows-portable.sh`（cargo-xwin）。产物核对项：
DMG 内 `.app` 的 `Contents/Resources/binaries/{mac/7zz, 7z-LICENSE.txt}` 存在、
`7zz` 哈希与 `binaries/mac/7zz` 一致、`Info.plist` 的版本 / `com.nanase.unparanoia` /
minOS 12.0 正确；Windows zip 的 7 个条目与 SHA256SUMS 自洽。注意 v0.2.0 曾因
`target/release/bundle/macos/` 残留半成品 dmg（512 B）与 `rw.<pid>.*.dmg` 导致
`bundle_dmg.sh` 报 `hdiutil convert` "文件已经存在"；清理残留后重跑即可，
详见 docs/BUILD.md §7。

`codesign --verify --deep --strict` 对 v0.3.0 的 .app **失败**（报
"code has no resources but signature indicates they must be present"），即本机产物为
ad-hoc 签名、不可直接分发。分发前必须用 Developer ID 重签并公证。

**GUI 启动冒烟（人工，2026-10-24 最终构建后由 Lead 执行，通过）**：
`open ./target/release/bundle/macos/Unparanoia.app` 可启动；窗口标题
「Unparanoia 解包器 — 批量解密/解压」、版本 0.3.0、识别内置 `7-Zip 25.01 (arm64)`；
tabs 为 队列/日志/密码/设置，密码区与配置路径显示正常，默认深色主题。
截图：`docs/screenshots/gui-0.3.0.png`（启动后即时截取，含新图标与默认深色主题）。
边界：仅启动级冒烟；交互式切页/拖放/暂停取消未做（尝试用 System Events 自动化时
accessibility 被系统拒绝，错误 -25208），**不等于**完整 GUI 交互测试（见 §9）。

## 8. Windows / Linux 状态

- Windows：**仅交叉编译验证**。`./scripts/build-windows-portable.sh` 在 v0.3.0 实测
  exit 0（2026-10-24 16:16）；产出
  `target/windows-portable/Unparanoia-0.3.0-win-x64-portable.zip`（5,855,710 B，见 §7），
  含 GUI exe、CLI exe、7z.exe/7z.dll/许可证/README/SHA256SUMS（7 个文件，用 `7zz l`
  核对过）。两个 exe 均为 PE32+ x86-64。**未在真实 Windows 上运行过**；
  NSIS 安装包未构建。详见 `docs/WINDOWS_BUILD.md`。
- Linux：未构建、未验证（Tauri 依赖 WebKitGTK 等，本仓库未提供 Linux 打包配置）。

## 9. 断言但未验证 / 不可复现清单（必须诚实保留）

| 项 | 状态 |
|---|---|
| 真实样本 MAC/checkcode/CRC/明文一致性 | 历史 Java Oracle 验证通过；**当前不可重跑**（缺 /tmp/paranoia-pc、/tmp/jdk21、原明文） |
| `test-vectors.json` 的 MAC/CRC 字段 | 是历史记录，Rust 测试不断言它们 |
| real_sample.rs 两个测试 | 默认 ignored；本机无样本，未执行 |
| GUI 手工交互（拖放、暂停/继续/取消、重试、密码管理页、主题切换） | 未做 GUI 自动化测试；仅保证 `pnpm build`、tauri 打包通过；最终构建启动冒烟通过（标题/版本/7-Zip/tabs/密码区/默认深色主题），但交互切页/拖放等未验证（System Events accessibility 被拒 -25208） |
| 密码库安全性（明文落盘） | 设计即明文 `config.json`（Unix 0600，Windows 无权限位）；这是有意的便利取舍，不是加密保险箱 |
| 逐层多密码重试的 GUI 路径 | CLI e2e 已覆盖（`layered.enc`）；GUI 只保证命令/界面存在，未做交互自动化 |
| Windows 真实运行、WebView2、中文路径 | 未验证 |
| DMG 安装体验/签名/公证 | 未签名、未公证；codesign 校验失败 |
| CI（.github/workflows/build.yml） | 仓库无远端，workflow 从未在 GitHub 上跑过；本地仅手工执行同等命令 |
| 7-Zip 二进制与官方下载的逐字节来源比对 | 未做（无网络）；只能靠版本字符串 + 记录哈希 + 继承来源 |
| 自解压 EXE 执行回退（`--allow-execute-sfx`） | 默认关闭，Windows-only，未验证 |
| PFTEREC1 恢复容器 | 明确不支持（v3 与恢复容器均未实现） |

## 10. 复现全部本机可复现项

```bash
cargo test --workspace
./scripts/make-fixtures.sh
./scripts/e2e-verify.sh
./scripts/build.sh
```
