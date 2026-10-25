# 合并映射：upzip → Unparanoia（v0.2.0 合并 + v0.3.0/v0.3.1/v0.3.2 增量）

> 本文记录 **upzip**（批量解压伪装压缩包，Tauri 2 + Rust）如何合并进
> **unparanoia**（Paranoia File Encryption v4 `.enc` 解密器），以及 v0.3.0 更名
> **Unparanoia（帕罗诺亚解包器）**、v0.3.1/v0.3.2 增量后的变化。upzip 源码位于
> /Volumes/Carrot/AppPorts/hermes/upzip，这里只记录迁移决策；代码本体已进入
> crates/unpack-core、crates/cli、gui/。
>
> 图例：**复用**（逻辑基本照搬，改名/改路径）· **改造**（接口或行为有实质变化）
> · **新增**（upzip 没有、由本项目实现）· **移除**（不迁移，并说明原因）。

## 1. 总览

v0.2.0 起的产品形态：**一个入口 = 批量解密 + 三层递归解包**（v0.3.0/v0.3.1 保持不变）。

1. 对 `.enc` 先做 Paranoia v4 解密（crates/core），得到内层 ZIP；
2. 对内层 ZIP、伪装/缺失文件头的压缩包、普通压缩包统一交给 7-Zip 解包；
3. 解出的内容若还是压缩包，继续递归（默认最多 4 层，可配 1..=8）；
4. 所有产物经暂存目录合并到目标目录，冲突按策略处理。

## 2. 模块级映射（v0.2.0 基线）

| upzip 模块 / 能力 | v0.2.0 位置 | 迁移 | 说明 |
|---|---|---|---|
| detect.rs：512 B 头 + 256 KiB 尾扫描、ZIP EOCD、PE overlay、7z l -slt 权威判定、切片刻离 | crates/unpack-core/src/detect.rs | 复用 | 判定顺序、container 取值、Detection 字段保留；新增 FileKind::SsefeEncrypted 与 container="ssefe" 分支 |
| detect.rs::carve_to_temp：伪装压缩包切片到临时文件 | 同上 | 复用 | 文件名 TMP_PREFIX 前缀；切片结果仍走 7-Zip |
| sevenzip.rs：locate/version/list/run/test_entry/extract、退出码 0/1/2/255、parse_slt | crates/unpack-core/src/sevenzip.rs | 改造 | 查找顺序改为 UNPARANOIA_7Z（兼容 UPZIP_7Z）→ exe 同名 → .app/Contents/Resources → 便携目录 → 开发仓库 binaries/ → PATH；其余复用 |
| queue.rs：输入展开、并发调度、Sink 事件回调 | crates/unpack-core/src/queue.rs | 改造 | 增加 stop_on_error；jobs 仍为并发上限 |
| engine.rs：单文件流水线 detect → extract → nested | crates/unpack-core/src/engine.rs | 改造 | 新增 SSEFE 分支：先 unparanoia_core::decrypt_to_archive 解到暂存 ZIP，再交给 7-Zip；嵌套深度用 max_depth；stage 字符串 detect/password/decrypt/extract/nested/land |
| layout.rs：Smart/Folder/Direct 布局、冲突策略、去垃圾文件、原子移动 | crates/unpack-core/src/layout.rs | 复用 | plan_dest/move_with_policy_landing/unique_sibling/strip_junk_tree 原样保留，规则见 [OUTPUT-LAYOUT.md](OUTPUT-LAYOUT.md) |
| cancel.rs：CancelToken、PauseGate | crates/unpack-core/src/cancel.rs | 复用 | CLI Ctrl-C 与 GUI 暂停/继续共用 |
| sfx.rs：Windows 自解压 exe 处理 | crates/unpack-core/src/sfx.rs | 复用（默认关闭） | allow_execute_sfx 仍默认 false，仅在 Windows 上有实际分支 |
| types.rs：Detection/TaskEvent/TaskOutcome/Summary/RunConfig/... | crates/unpack-core/src/types.rs | 改造 | 去 vault 相关字段；RunConfig 增加 max_depth、stop_on_error、processed_path；TaskOutcome 增加 warnings；FileKind 增加 ssefe_encrypted |
| error.rs：UpError | crates/unpack-core/src/error.rs | 复用 | 变体不变；SSEFE 错误包装为 Other/字符串 |
| util.rs：指纹、临时文件、原子写、空间检查、sanitize_component | crates/unpack-core/src/util.rs | 复用 | 新增 TMP_PREFIX = ".unparanoia-tmp-" 的用途约定 |
| vault.rs：密码库（keyring/加密文件、条目、绑定、processed 记录） | — | **移除** | 见 §3；密码记忆改为「单个明文密码」，processed 记录独立成 processed.rs |
| settings（upzip 在 lib.rs 内） | crates/unpack-core/src/settings.rs | 新增/改造 | 独立 Settings 结构（serde camelCase），settings.json 与 core 的 config.json 同目录；v0.3.0 增加 theme |
| processed 跳过记录（upzip 在 vault 内） | crates/unpack-core/src/processed.rs | 新增/改造 | ProcessedStore → processed.json，按输出文件仍存在判定「已处理」 |
| SSEFE 探测/解密 | crates/unpack-core/src/ssefe.rs + crates/core | **新增** | enc_probe -> Option<SsefeProbe>、decrypt_to_temp；实际解密复用 unparanoia-core |
| CLI（upzip crates/upzip-cli：doctor/detect/run/pw/cleanup） | crates/cli（bin unparanoia-cli） | 改造 | 去 vault 子命令；新增 inspect、config、cleanup-staging、--json JSON Lines；旧 .enc 用法保持兼容，见 [DEV-CONTRACT.md](DEV-CONTRACT.md) §3 |
| Tauri GUI（upzip src-tauri + React） | gui/ | 改造 | 事件改为 unparanoia://event / unparanoia://done；命令集合按 DEV-CONTRACT §4；前端以 upzip App.tsx 为底稿删改 |
| 构建脚本 / CI | scripts/ + .github/workflows/build.yml | 新增/改造 | 见 [BUILD.md](BUILD.md)、[WINDOWS_BUILD.md](WINDOWS_BUILD.md) |
| 7-Zip 二进制与许可 | binaries/ + binaries/README.md + NOTICE | 复用 | 版本 25.01，sha256 记录；LGPL + unRAR 限制，详见 NOTICE |
| 测试夹具 | scripts/make-fixtures.sh | **新增** | 仓库内可再生成；make_enc（testkit）生成 .enc；v0.3.0 增加 layered.enc 两级不同密码 |
| 独立 Java oracle | verification/oracle/ | 保留（来自 v0.1.x） | 非 Rust 的第二实现 + test-vectors.json；见 [VERIFICATION.md](VERIFICATION.md) |

## 3. 为什么 v0.2.0 移除 upzip 的 vault、v0.3.0 又加了密码库

upzip 的 vault.rs 提供：主密钥存 OS keychain 或 vault.key（声称 0600）、
vault.enc 加密条目、密码-归档绑定、使用频次排序、processed 记录。v0.2.0 移除：

- **安全收益小于复杂度**：vault.key 的 0600 只在 cfg(unix) 生效（Windows ACL
  未实现），同一用户可读；把「密码猜测排序」包装成安全特性会误导用户。
- **需求只剩一个**：Paranoia 场景通常一个用户密码；记忆功能 = 明文密码 +
  可选勾选（与 v0.1.x config.json 行为一致，显式告知用户）。
- **减少 GUI/CLI 合同面**：无 vault 事件、无 vault 命令、无 keyring 依赖。

v0.3.0 的用户需求是「独立密码管理页、多密码明文保存」，因此重新引入**密码库**，
但与 upzip vault 本质不同：

| | upzip vault | Unparanoia 0.3.0 密码库 |
|---|---|---|
| 存储 | vault.enc 加密文件 + keychain/vault.key 主密钥 | `config.json` 明文 `{"passwords":[...]}`（Unix 0600） |
| 依赖 | keyring / AES 加密 / 迁移逻辑 | 无额外依赖，core::config 直接读写 |
| 绑定/频次 | 密码-归档绑定、按使用频次排序猜测 | **没有**；只是有序候选列表 |
| 用途 | 自动猜密码 | 用户显式维护的明文口令列表；每层解包都会重试 |

即：v0.2.0 移除的是「伪安全的自动猜测库」，v0.3.0 加回的是「透明的明文密码清单」。

## 4. 迁移后的产品差异（相对 upzip）

| 能力 | upzip | v0.2.0 | v0.3.0 |
|---|---|---|---|
| 输入 | 压缩包/伪装包/SFX | 上述全部 **+ Paranoia v4 .enc（SSEFE）** | 不变 |
| 密码 | 密码库 + 频次 | 单个密码 + 可选记忆 + 每次任务可传多密码 | **多密码明文密码库 + 每层完整候选池重试** |
| SSEFE | 无 | 先解密再解压，内层 ZIP 支持递归 | 不变 |
| 默认嵌套深度 | 4 | 4（1..=8 可配） | 不变 |
| 失败策略 | 跳过并继续 | stop_on_error 可配；默认继续 | 不变 |
| 输出布局 | smart/folder/direct | 相同（规则见 OUTPUT-LAYOUT.md） | 不变 |
| 冲突重命名 | `hello_1.txt`（`_<n>`） | **不采用 upzip 风格**，沿用旧 paranoia 0.1.x 的 `hello (1).txt`（` (<n>)`） | 不变 |
| legacy CLI 位置参数 | — | 兼容 0.1.1 用法，且额外读取「记住的口令」（旧版只认 `-p`/环境变量） | 额外读取密码库全部 |
| GUI | 4 标签页含密码库 | 单页批量 + 设置；状态/警告/重试 | 队列/日志/密码/设置 4 标签页；明暗主题 |
| 平台 | macOS 已验证 | macOS 优先；Windows 仅交叉编译，未真机验证 | 不变 |

### 4.1 两处有意为之的行为选择

1. **冲突重命名保留旧 paranoia 风格**：旧 paranoia 0.1.x 生成 `hello (1).txt`；
   upzip 基线生成 `hello_1.txt`。合并后在 `layout::unique_sibling` 中统一为
   `hello (1).txt` / 目录 `pkg (1)`（`crates/unpack-core/src/layout.rs:104-126`，
   tests 也在断言该格式）。目的是减少既有 paranoia 用户的回归；若依赖 upzip
   的下划线命名需自行改名。
2. **legacy CLI 也读记住的口令/密码库**：位置参数用法（旧 0.1.1 风格）在 `-p` 与环境变量
   都没有时，会读取配置里的口令；旧版不会。属契约内增强。

## 5. v0.3.0 增量（本次更名版本）

| 需求 | 实现位置 | 说明 |
|---|---|---|
| 全仓更名 Unparanoia | Cargo.toml / crates/* / gui/* / scripts / docs | 包名 unparanoia / unparanoia-core / unparanoia-unpack-core / unparanoia-cli；bundle id com.nanase.unparanoia；事件 unparanoia://* |
| 密码管理页（多密码明文） | gui/src/App.tsx（密码标签页）+ crates/core/src/config.rs + gui/src-tauri/src/lib.rs | `config.json` 的 `passwords` 数组；`list_passwords`/`add_password`/`remove_password`/`clear_passwords` 命令 |
| CLI 密码库管理 | crates/cli/src/main.rs（config 子命令） | `--list-passwords` / `--add-password` / `--remove-password` / `--clear-passwords`，保留旧 `--show`/`--path`/`--set-password`/`--clear-password` |
| 逐层多密码重试 | crates/unpack-core/src/engine.rs | 候选池 = RunConfig.passwords → 归档注释 hints → 空密码；SSEFE WrongPassword 与 7z PasswordRequired 都继续尝试剩余候选；每层重走完整候选池 |
| 明暗主题可跟随系统 | crates/unpack-core/src/settings.rs（ThemeMode）+ gui/src/App.tsx/App.css + gui/src-tauri | 设置页下拉 跟随系统/浅色/深色；`document.documentElement.dataset.theme`；system 监听 `prefers-color-scheme` |
| 重画图标 | assets/icon.svg + gui/src-tauri/icons/* + gui/public/icon.png | 蓝渐变圆角方 + 白色开盖纸箱/拉链 + 金色开锁挂锁 |
| 旧配置迁移 | crates/core/src/config.rs、crates/unpack-core/src/settings.rs | 旧 `paranoia-enc-decryptor/` 目录只读迁移到 `unparanoia/`；旧 env `PARANOIA_ENC_*` 仅回退读取 |
## 6. v0.3.1 增量（分卷 / Windows 单 exe）

| 需求 | 实现位置 | 说明 |
|---|---|---|
| 分段压缩包（普通分卷） | `crates/unpack-core/src/split.rs` + `engine.rs` | 识别 `.7z.001`/`.zip.001`/`.part1.rar` 家族；同族聚合为一个任务；7-Zip 只从首卷进入；只给非首卷 → Skipped + 「请添加第一卷」；缺后续卷 → Failed + 零半成品 |
| SSEFE 加密分卷（`.enc`） | 同上 + `ssefe.rs` | 每卷是独立 Paranoia v4 容器；逐卷用完整候选池解密到 staging `volumes/<stem>/`，再按 7-Zip 分卷读取；任一卷失败不产出半成品 |
| 真实样本验证 | — | `3138.7z.001.enc`+`.002.enc`（口令 123/anzixin）→ 1 任务、输出 `3138/` 5 文件 352.85 MiB；只给首卷自动发现、只给非首卷跳过、缺卷失败（VERIFICATION.md §8） |
| Windows 单 exe（内嵌 7-Zip） | 新增 `crates/embedded-7z/` + `gui/src-tauri/src/lib.rs` + `crates/cli/src/main.rs` | Windows 版 `include_bytes!` 内嵌 7z.exe/7z.dll/许可证；首次运行解压到 `%LOCALAPPDATA%\Unparanoia\embedded-7z\25.01-r1\`（`UNPARANOIA_7Z_CACHE_DIR` 可覆盖）；GUI 回退不阻止启动，CLI 优先级 explicit > env > 内嵌 > 自动探测 |
| 7-Zip 许可证查看 | `crates/cli/src/main.rs`（`--licenses`）+ GUI 设置页 | 内嵌/随包 7-Zip 的 LGPL 2.1 + unRAR 限制文本；不改变许可义务 |
| 修复 Windows GUI 白屏 | `gui/src-tauri/Cargo.toml` 的 `custom-protocol` feature + 构建脚本/CI | 裸 cargo build 缺 `--features custom-protocol` 时不嵌入前端、运行时加载 devUrl 导致白屏；所有 Windows GUI 构建路径必须显式带该 feature |
| 便携包不再带 sidecar | `scripts/build-windows-portable.sh`/`build-windows.ps1`/CI | 输出 `Unparanoia.exe` + `unparanoia-cli.exe` + 许可证/README/SHA256SUMS（单 exe 可独立运行）；不再复制 7z.exe/7z.dll |
| 测试夹具与 e2e | `scripts/make-fixtures.sh`、`scripts/e2e-verify.sh` | 新增 `split.7z.001..004`、`split.7z.001.enc..`、`truncated.7z.001.enc`；e2e 增加普通分卷/加密分卷组/只给非首卷/缺卷 + `--licenses` 用例（91 项） |
| 版本与产物命名 | Cargo.toml / gui/package.json / tauri.conf.json / docs | 0.3.1；`Unparanoia_0.3.1_aarch64.dmg`、`Unparanoia-0.3.1-win-x64-portable.zip`；最终哈希见 VERIFICATION.md §7 |
## 7. v0.3.2 增量（跳过默认关闭 / Windows 体验 / 日志诊断）

| 需求 | 实现位置 | 说明 |
|---|---|---|
| 跳过已处理默认关闭 | `crates/unpack-core/src/types.rs`（`RunConfig::default`）+ `settings.rs`（`settingsVersion` 迁移）+ `crates/cli/src/main.rs`（`--skip-processed`） | v0.3.1 默认开启导致真实用户的分卷输入被静默跳过；v0.3.2 默认不跳过、不写 `processed.json`，旧 `settings.json` 一次性迁移为关闭；CLI 改为 `--skip-processed` opt-in（旧 `--no-skip-processed` 兼容 no-op） |
| 分卷队列行归并 | `crates/unpack-core/src/types.rs`（`TaskOutcome.related_paths`）+ `engine.rs` + `gui/src/App.tsx` | 同族卷在 GUI 队列归并为一行；全跳过批次显示提示条（清除处理记录/全部重新解压） |
| Windows 无控制台闪窗 + 错误细节 | `crates/unpack-core/src/sevenzip.rs` | Windows `CREATE_NO_WINDOW`；失败信息含 7z 路径/退出码/stderr 尾部（≤1000 字符）；spawn 锁文件错误 5/32/225 重试 3 次 |
| 日志落盘 + 诊断 | `gui/src-tauri/src/lib.rs`（`unparanoia.log`、`append_log`、`diagnostics`）+ `gui/src/App.tsx` | 日志 `<configDir>/logs/unparanoia.log`（>2 MiB 轮转）；设置页「诊断信息」（复制诊断 JSON/打开日志目录/清除处理记录） |
| 测试与产物 | `scripts/e2e-verify.sh` + docs | e2e 91→104（新增默认不跳过/--skip-processed/related_paths/清除记录后重跑）；测试 107→114（unpack-core 65 + GUI lib 2）；产物 `Unparanoia_0.3.2_aarch64.dmg` / `Unparanoia-0.3.2-win-x64-portable.zip`，哈希见 VERIFICATION.md §7 |


