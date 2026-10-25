# 开发契约 (v0.3.1)

本文件是并行开发期间的**唯一接口契约**。Lead 维护；core-reader / app-reader / gui-frontend / docs-reader
按各自写作用域实现，不要修改别人作用域内的文件（需要变更先发给 Lead）。

## 1. 仓库布局与归属

| 路径 | 内容 | Owner |
| --- | --- | --- |
| `crates/core/**` | unparanoia-core：SSEFE v4 密码学核心（原有）+ 新增 `decrypt_to_archive` / `verify_password` / `testkit` | Lead |
| `crates/unpack-core/**` | unparanoia-unpack-core：从 upzip-core 迁入的批量解压引擎，去 vault、加 SSEFE、明文 processed 存储 | Lead |
| `crates/cli/**` | unparanoia-cli 多格式批量 CLI（从 upzip-cli 改造，保留旧 paranoia CLI 兼容面） | core-reader |
| `gui/src-tauri/**` | Tauri v2 后端（从 upzip src-tauri/lib.rs 改造，明文密码/设置） | app-reader |
| `gui/src/**`、`gui/index.html`、`gui/vite.config.ts`、`gui/tsconfig*.json`、`gui/public/**`（可选）、`gui/package.json` 的依赖/脚本 | React 19 + TS 前端（从 upzip App.tsx 改造） | gui-frontend |
| `docs/**`、`README.md`、`scripts/**`、`LICENSE`、`NOTICE`、`binaries/**`、`.github/workflows/**` | 文档、构建/验证脚本、第三方许可与二进制台账 | docs-reader |
| `Cargo.toml`、`Cargo.lock`、`.gitignore`、`testdata/` fixtures | 工作区、锁文件、清理规则、验收样例 | Lead |

根工作区成员：`crates/core`、`crates/unpack-core`、`crates/cli`、`gui/src-tauri`；版本 0.3.0（产品名 Unparanoia）。
**v0.3.0 的全部接口变更见第 7 节；v0.3.1 见第 8 节；第 2–7 节凡与第 8 节冲突，以第 8 节为准。**
**不要**给 gui/src-tauri 再加 `[workspace]`；不要提交 `target/`、`gui/node_modules/`、`gui/dist/`、`gui/src-tauri/binaries/`、`testdata/`。

构建配置变更（相对 0.1.1，已记录）：release `opt-level = 3`（原 "s"）、移除 `panic = "abort"`（GUI 批量线程需要 catch_unwind 兜底）；保留 dev 下 argon2/skein/blake3/sha3 = 3。

## 2. 引擎 API（unparanoia-unpack-core，已冻结）

crate 名 `unparanoia-unpack-core`，lib 名 `unparanoia_unpack_core`。公开导出：

```rust
pub const TMP_PREFIX: &str = ".unparanoia-tmp-";

pub use cancel::{CancelToken, PauseGate};
pub use detect::detect;
pub use engine::Engine;
pub use error::{Result, UpError};
pub use layout::{archive_stem, plan_dest, strip_junk_tree};
pub use queue::{expand_inputs, run_batch, Sink};
pub use sevenzip::{PasswordMode, PwCheck, SevenZip};
pub use settings::Settings;
pub use types::*; // ConflictPolicy/LayoutPolicy/FileKind/Detection/TaskStatus/TaskOutcome/TaskEvent/Summary/RunConfig...
pub use processed::ProcessedStore;
pub use ssefe::enc_probe; // Option<SsefeProbe>
```

### types 变化（相对 upzip）

- `FileKind` 新增 `SsefeEncrypted`（serde `"ssefe_encrypted"`）。
- `TaskOutcome` 新增 `pub warnings: Vec<String>`（嵌套失败/路径安全警告；默认空）。
- `RunConfig` 删除 `remember_hints`、`use_vault`；新增：
  - `pub max_depth: usize`（默认 4，clamp 1..=8）
  - `pub stop_on_error: bool`（默认 false；true 时首个 Failed/NeedsPassword 后整批停止）
  - `pub processed_path: Option<PathBuf>`（None = 配置目录/processed.json）
- `RunConfig` 其余字段与 upzip 相同（output_root/conflict/layout/jobs/keep_broken/try_empty_password/skip_processed/strip_junk/nested/recursive/dry_run/force_documents/allow_execute_sfx/passwords/min_free_ratio）。
- `TaskStatus`/`TaskEvent`/`Summary`/`Detection` 结构不变（Detection 描述 SSEFE 时：kind=SsefeEncrypted、
  format="paranoia-enc-v4"、container="ssefe"、notes 含 "Paranoia File Encryption v4 (SSEFE) AES-256"）。

### Engine

```rust
impl Engine {
    pub fn new(sz: SevenZip, cfg: RunConfig) -> Self;
    pub async fn process_file(&self, id: u64, path: &Path, cancel: &CancelToken,
                              gate: &PauseGate, emit: &mut dyn FnMut(TaskEvent)) -> TaskOutcome;
    pub async fn save_processed(&self);
}
pub async fn run_batch(engine: Arc<Engine>, files: Vec<PathBuf>, cancel: CancelToken,
                       gate: Arc<PauseGate>, sink: Sink) -> Summary;
pub fn expand_inputs(inputs: &[PathBuf], recursive: bool, exclude: Option<&Path>) -> Vec<PathBuf>;
```

### SevenZip 定位（`SevenZip::locate(None)`）

顺序：env `UNPARANOIA_7Z`（兼容 `UPZIP_7Z`）→ exe 同目录 `7zz`/`7z` → macOS .app
`../Resources/binaries/mac/7zz` → Windows exe 同目录/便携包 `7z.exe` 与 `binaries/win/7z.exe` → dev 仓库根
`binaries/mac/7zz` / `binaries/win/7z.exe`（从 exe 路径向上找 6 层）→ PATH `7zz`/`7z`。

### 明文配置 / 状态

- 记住的密码：沿用 unparanoia-core 的 `config.json`（明文；`UNPARANOIA_CONFIG_DIR` 可覆盖，默认用户配置目录
  `unparanoia/`）。API：`load_remembered_password/save_remembered_password/clear_remembered_password/config_file_path`。
- 设置：同目录 `settings.json`，明文，由 `unparanoia_unpack_core::settings` 读写（见 `settings.rs`，serde camelCase）。
- 已处理记录：默认同目录 `processed.json`，明文；`ProcessedStore` 由 Engine 持有，`forget_processed` 仅删除该文件。

```rust
pub struct Settings { // serde camelCase，缺省值见 Default
    pub output_root: Option<PathBuf>, pub jobs: usize, pub conflict: ConflictPolicy,
    pub layout: LayoutPolicy, pub skip_processed: bool, pub force_documents: bool,
    pub keep_broken: bool, pub strip_junk: bool, pub nested: bool, pub max_depth: usize,
    pub stop_on_error: bool, pub try_empty_password: bool, pub allow_execute_sfx: bool,
    pub recursive: bool, pub min_free_ratio: f64,
}
impl Settings {
    pub fn to_run_config(&self, passwords: Vec<String>, processed_path: Option<PathBuf>) -> RunConfig;
    pub fn load() -> Settings;          // 读取失败回退 Default，不 panic
    pub fn save(&self) -> anyhow-free Result<(), String>;
    pub fn path() -> Option<PathBuf>;
}
```

## 3. CLI 契约（crates/cli，owner core-reader）

二进制名 `unparanoia-cli`。**旧的 0.1.1 用法保持可用**（位置参数 = 待解密 .enc 文件，`-p/--password`、
`-o/--output-dir`、`--conflict`、`--json`、`--inspect`、`--config-path`、`--clear-password`）。

新命令（子命令优先）：
- `unparanoia-cli [PATH...] [OPTS]`（无子命令）= 批量解压所有路径（等价 run）
- `run [PATH...]`：`-o/--output-dir DIR`、`-j/--jobs N`、`-p/--password PW`（可重复；也读 env
  `UNPARANOIA_PASSWORD`）、`--conflict rename|skip|overwrite`、`--layout smart|folder|direct`、
  `--no-recursive`、`--no-nested`、`--max-depth N`、`--keep-broken`、`--dry-run`、`--force-documents`、
  `--allow-execute-sfx`、`--stop-on-error`、`--no-skip-processed`、`--no-strip-junk`、`--try-empty-password`、`--json`
- `detect PATH...`：只识别（JSON 输出 Detection；人读输出 kind/format/密码状态）
- `inspect FILE.enc`：旧功能，打印 .enc 头（版本/算法/customParams/Argon2 参数）
- `config`：`--show`（是否已保存、路径；绝不打印密码本身）/ `--path` / `--set-password PW` / `--clear-password`
- `doctor`：7z 位置与版本、配置目录、settings/processed 路径、平台
- `cleanup-staging [--root DIR]...`：删除 `.unparanoia-tmp-*` 残留（默认扫描当前目录/参数根），打印数量
- `--version` / `--help`

行为：
- 密码来源：`-p` > env `UNPARANOIA_PASSWORD` > 明文 config.json 中记住的密码；全部进入 `RunConfig.passwords`
  （去重、空串剔除）。
- 批量控制：jobs 默认 2；`--stop-on-error`；Ctrl-C 触发 cancel（退出码 130）；汇总打印
  total/success/skipped/needs_password/failed/cancelled/bytes/elapsed。
- 失败/需密码任务逐条打印明确中文原因；退出码：0=全部成功或跳过；1=存在 failed 或 needs_password；2=用法/配置/环境错误；130=中断。
- `--json`：stdout 输出**纯 JSON Lines**：先逐条 TaskEvent（引擎 serde 原样 JSON，含 `type` 字段），
  最后一行 `{"type":"summary", ...Summary}`；人类日志走 stderr。stdout 不得混入其他内容。
- 旧 `--inspect`：单文件，输出同 inspect（人类可读或 --json 对象），不做解密。
- `--config-path` 打印 config.json 路径；`--clear-password` 删除记住的密码（幂等）。
- 旧 `--json` 单文件输出包含 `outputs/originalName/bytesWritten` 的 JSON 对象（兼容 0.1.1 脚本）。

## 4. GUI 契约（Tauri 后端 owner app-reader；前端 owner gui-frontend）

- 事件：`unparanoia://event`（payload = TaskEvent serde JSON），`unparanoia://done`（payload = Summary）。
- 命令（`gui/src-tauri/src/lib.rs` `invoke_handler` 注册；serde camelCase）：

```rust
app_info() -> AppInfo // {version, configDir, configPath, settingsPath, sevenZip: Option<String>, sevenZipVersion: Option<String>, stagingPrefix}
startup_paths() -> Vec<String>                 // 进程启动参数里的文件（冷启动 open-with argv）
expand_paths(paths: Vec<String>) -> Vec<String> // 目录递归展开、去重、排序（用 expand_inputs，exclude=output_root）
inspect_file(path: String) -> Result<Detection, String>
start_batch(req: StartRequest) -> Result<(), String>
batch_state() -> BatchState                    // {running, paused, summary: Option<Summary>}
pause_batch() -> Result<(), String>; resume_batch() -> Result<(), String>; cancel_batch() -> Result<(), String>
settings_get() -> Settings; settings_set(settings: Settings) -> Result<(), String>
load_remembered_password() -> Result<Option<String>, String>
save_remembered_password(password: String) -> Result<(), String>
clear_remembered_password() -> Result<(), String>
get_config_path() -> Result<String, String>
pick_input_files() -> Vec<String>; pick_output_dir() -> Option<String>    // async，用 tauri-plugin-dialog
reveal_path(path: String) -> Result<(), String>
cleanup_staging(roots: Vec<String>) -> Result<usize, String>
forget_processed() -> Result<usize, String>    // 删除 processed.json，返回删除记录数（0 若不存在）
```

```rust
pub struct StartRequest { pub files: Vec<String>, pub password: Option<String>,
                          pub remember_password: bool, pub settings: Settings }
```

- start_batch：单飞（running 时拒绝）；在后台线程/任务用 `run_batch` 串联事件；**必须** catch_unwind/错误兜底，
  无论发生什么最后都 emit `unparanoia://done`（payload Summary）并把 running 复位。密码：request.password 非空则用；
  否则尝试 load_remembered_password()；remember_password=true 时把 request.password 明文写 config.json。
- 批量完成后保存 Settings（settings_set 已在运行前保存亦可）、`engine.save_processed()`。
- 拖放/选择目录：前端调用 expand_paths。
- Windows reveal：只把 explorer 退出码 0/1 都视为成功（避免误报）。
- 不需要 token 鉴权；webview 不联网（tauri 允许 dialog 插件即可；capabilities 只保留 core/dialog 权限）。
- 删除 `commands.rs`/`state.rs` 旧 paranoia 实现；不要引用旧事件名 `batch-progress` 等。

## 5. 前端契约（gui/src，owner gui-frontend）

- 技术栈：React 19 + TS + Vite（沿用已复制进来的 upzip App.tsx/App.css/main.tsx/index.html/vite.config.ts/tsconfig）。
- 必做改造：
  1. 删除密码库（vault）相关 UI/类型/调用；v0.3.0 改为独立「密码管理」页 + 可选本次追加密码（见第 7 节，取代原单一密码框方案）。
  2. 任务类型/阶段标签支持 `ssefe_encrypted`（例如「.enc 加密容器」）与阶段 `decrypt`（「解密 SSEFE」）、
     `extract`（「解压」）、`nested`（「递归内层」）、`land`（「写出文件」）。
  3. 结果面板：状态含 Success/Skipped/NeedsPassword/Failed/Cancelled；展示 reason 与 `warnings`；
     失败/需密码条目可「重试失败项」（用当前文件列表重新 start_batch，不新造后端状态）。
  4. 设置面板字段 = 第 2 节 Settings 的 camelCase；jobs 1..8；max-depth 1..8；stopOnError 开关；outputRoot 选择；
     「清理临时残留」按钮（cleanup_staging，默认当前输出根/临时目录）；「清除处理记录」按钮（forget_processed）。
  5. 批量控制：暂停/继续/取消按钮；进度条按 TaskProgress.percent，未知时用 summary 计数；日志面板保留 Log 事件。
  6. 启动时：app_info → settings_get → list_passwords → startup_paths → expand_paths → 填充列表（v0.3.0，见第 7 节）。
  7. 全部文本用 textContent/React 文本，不用 innerHTML；无外部网络请求；无 XSS 面。
- `pnpm --dir gui build`（tsc + vite build）必须通过。

## 6. 构建/产物契约（scripts + docs，owner docs-reader）

- `./scripts/build.sh`：cargo test --workspace → pnpm install（frozen-lockfile 不存在则普通 install）→
  `pnpm --dir gui tauri build`；打印产物：`target/release/bundle/dmg/*.dmg`、`target/release/bundle/macos/*.app`。
- `./scripts/build-windows-portable.sh`：pnpm --dir gui build（前端 dist）→ cargo xwin build --release
  --target x86_64-pc-windows-msvc（GUI + CLI）→ 打包
  `target/windows-portable/Unparanoia-0.2.0-win-x64-portable.zip`（exe + 7z.exe + 7z.dll + 7z-LICENSE.txt +
  README + CLI exe + SHA256SUMS）。不依赖 NSIS/安装器；WebView2 由 Windows 11 自带。未在真机验证须写清楚。
- `scripts/sync-binaries.mjs` 路径从 repo 根计算（现在是 `../scripts` 被 gui 调用；改成以自身位置解析 repo 根，
  pairs 目标 `gui/src-tauri/binaries/**`）。
- `scripts/make-fixtures.sh`（或 .mjs）：在仓库内 `testdata/fixtures/` 生成 normal.zip、encrypted.7z（密码 123）、
  nested.zip、masked .mp4（zip 加前缀）、sample.enc（用 `cargo run -p unparanoia-core --example make_enc --`）、
  nested.enc；全部仓库内可重建。
- `scripts/e2e-verify.sh`：用 CLI 跑上述 fixtures，断言输出文件/退出码/JSON，尾行 `ALL E2E CHECKS PASSED`。
- 文档：`docs/MERGE-MAPPING.md`（upzip 功能清单 ↔ 复用/改造/新增 + 缺失说明）、`docs/OUTPUT-LAYOUT.md`（布局/冲突/
  进度/日志/临时文件/失败策略）、`docs/BUILD.md`（mac/Windows、依赖、产物、诚实边界）、`docs/WINDOWS_BUILD.md`、
  `docs/VERIFICATION.md`（最终验证记录，由 Lead 给数据）、README（Lead 终稿可引用）、`LICENSE`（MIT）、
  `NOTICE`（7-Zip LGPL 25.01 + unRAR 限制 + paranoia 原有说明）、`binaries/README.md`（sha256/版本/来源 URL/许可）。
- 7-Zip 来源：官方 https://www.7-zip.org/a/7z2501-mac.tar.xz 与 https://www.7-zip.org/a/7z2501-x64.exe（或 extra 包）；
  若无法联网核对，必须标注「二进制沿用自 upzip 仓库，hash 已记录，官方下载未复核」。

## 7. v0.3.0 增补（2026-10-24，最新契约，覆盖前文冲突处）

产品正式更名 **Unparanoia**（中文副题「帕罗诺亚解包器」）：GUI 包/二进制 unparanoia、lib unparanoia_lib、
CLI unparanoia-cli、crates unparanoia-core / unparanoia-unpack-core、bundle id com.nanase.unparanoia、
配置目录 unparanoia、env UNPARANOIA_CONFIG_DIR / UNPARANOIA_PASSWORD / UNPARANOIA_7Z、事件
unparanoia://event / unparanoia://done、临时前缀 .unparanoia-tmp-。版本 **0.3.0**。
兼容：旧 env PARANOIA_ENC_CONFIG_DIR / PARANOIA_ENC_PASSWORD / PARANOIA_7Z / UPZIP_7Z 与旧配置目录
paranoia-enc-decryptor 仅作**回退读取/迁移**，新写入一律用新名字。SSEFE/Paranoia File Encryption v4 等格式名
与 PARANOIA_SAMPLE_* / PARANOIA_ORACLE_VECTORS / /tmp/paranoia-pc 不改。

### 7.1 core config：多密码明文存储（Lead 实现，crates/core/src/config.rs）

文件仍是明文 JSON（unix 0600）：

```json
{ "passwords": ["123", "family-share"] }
```

公开 API（crates/core）：

```rust
pub const CONFIG_FILE_NAME: &str = "config.json";
pub const CONFIG_DIR_NAME: &str = "unparanoia";
pub const LEGACY_CONFIG_DIR_NAME: &str = "paranoia-enc-decryptor";

pub fn config_dir() -> Result<PathBuf, ConfigError>;        // UNPARANOIA_CONFIG_DIR > PARANOIA_ENC_CONFIG_DIR(兼容) > 默认
pub fn config_file_path() -> Result<PathBuf, ConfigError>;
pub fn legacy_config_dir() -> Option<PathBuf>;
pub fn load_passwords() -> Result<Vec<String>, ConfigError>;   // 去空、去重、保序
pub fn save_passwords(passwords: &[String]) -> Result<(), ConfigError>;
pub fn add_password(password: &str) -> Result<bool, ConfigError>;   // 已存在返回 Ok(false)，不加空串
pub fn remove_password_at(index: usize) -> Result<bool, ConfigError>;
pub fn clear_passwords() -> Result<(), ConfigError>;
// 兼容旧调用（内部转调多密码 API）：
pub fn load_remembered_password() -> Result<Option<String>, ConfigError>;  // 返回第一个
pub fn save_remembered_password(password: &str) -> Result<(), ConfigError>; // 用单元素列表覆盖
pub fn clear_remembered_password() -> Result<(), ConfigError>;
```

迁移规则：新 config.json 不存在而旧目录 <legacy>/config.json 存在时，读取旧的 rememberedPassword（或旧结构里
的 remembered_password）与 passwords，去重后写入新文件；旧文件不删除。settings.json 同样：新目录无
settings.json 而旧目录有时，由 Settings::load() 回退读取旧文件（再次保存写新位置）。

### 7.2 Settings 新字段（engine-dev）

```rust
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    // ...原有 15 个字段不变...
    pub theme: ThemeMode,   // 新增；不参与 to_run_config
}
pub enum ThemeMode { System, Light, Dark }  // serde lowercase: "system"|"light"|"dark"，Default=System
```

### 7.3 引擎：逐层多密码尝试（engine-dev，crates/unpack-core/src/engine.rs）

密码候选池（按顺序、去重）：RunConfig.passwords（GUI = 明文密码库全部 + 本轮追加；CLI = -p 列表 + env +
密码库全部）→ 归档注释 hints（source="hint"）→ 空密码（仅 try_empty_password，source="empty"）。每个候选在
TaskPasswordAttempt{id,source,ok,tried} 中如实上报。

必须修复的两处提前放弃（v0.2.0 缺陷）：
1. 主解压（sz.extract）返回 PasswordRequired 时：**继续尝试剩余候选密码**重新解压，而不是直接
   NeedsPassword；全部候选失败才返回 NeedsPassword。
2. SSEFE 分支 ssefe::decrypt_to_temp 返回 WrongPassword 时：**继续尝试剩余候选**，全部失败才 NeedsPassword。
嵌套层（既有行为）保持：每展开一层都重新走完整候选池（内层与外层密码不同 → 新密码仍能命中）。
某文件用非第一候选成功时，TaskOutcome.warnings 追加「使用候选密码 #N 成功」。取消/暂停语义不变。

### 7.4 GUI 命令与启动契约（app-reader，gui/src-tauri/src/lib.rs）

删除 load_remembered_password / save_remembered_password / clear_remembered_password。新增：

```text
list_passwords() -> Vec<String>                      // 明文列表，仅返回给本机 webview
add_password(password: String) -> Result<bool, String>
remove_password(index: usize) -> Result<bool, String>
clear_passwords() -> Result<(), String>
```

StartRequest { files: Vec<String>, password: Option<String>, remember_password: bool, settings: Settings }
沿用，但语义：password = 本轮**追加/一次性**密码（为空则不加）；remember_password=true 且 password 非空 →
core::add_password 持久化。start_batch 组装的 RunConfig.passwords = load_passwords() + 追加密码（去重，追加在前）。
get_config_path 保留（指向新 config.json）。设置保存含 theme；事件名已是 unparanoia://event / unparanoia://done。
图标：bundle.icon 使用 icons/32x32.png、icons/128x128.png、icons/128x128@2x.png、icons/icon.icns、icons/icon.ico
（Lead 生成后到位）。

### 7.5 前端（gui-frontend，gui/src/App.tsx、App.css、index.html）

- Tabs 变为 **队列 / 日志 / 密码 / 设置** 四个。新增「密码」页：密码列表（安全掩码显示，可眼睛切换）、
  添加输入框+按钮、单项删除、清空全部、显示明文存放路径（get_config_path）与「明文保存、仅存本机」说明。
- 队列页密码区改为：可选「本次追加密码（仅本次运行）」输入框 + 「记住到密码库」勾选；不再有单密码记忆逻辑；
  开始任务调用 start_batch 时传 password 与 remember_password。
- 设置页新增「外观主题」下拉：跟随系统 / 浅色 / 深色（写入 settings.theme）；前端用
  document.documentElement.dataset.theme = "light"|"dark"，system 时监听 matchMedia("(prefers-color-scheme: dark)")。
  App.css 现有深色变量为默认；新增 [data-theme="light"] 覆盖，保证队列/日志/密码/设置/对话框可读。
- 标题/页头使用新品牌 Unparanoia + 应用图标（/icon.png 或内联 SVG，Lead 生成到 gui/public/icon.png）；
  gui/index.html title 已改。启动顺序改为 app_info → settings_get → list_passwords → startup_paths → expand_paths。
- pnpm --dir gui build 必须通过；无 innerHTML 注入；不联网。

### 7.6 CLI（core-reader，crates/cli/src/main.rs）

- config 子命令扩展（保留 --show / --path / --clear-password 兼容）：
  --list-passwords、--add-password <PW>、--remove-password <INDEX>（0 基）、--clear-passwords；
  --show 打印密码库（掩码或明文由实现定，文档写清）。
- 密码来源全量：-p/--password（可重复）→ env UNPARANOIA_PASSWORD（兼容 PARANOIA_ENC_PASSWORD）→
  config 密码库全部 →（引擎还会加 hints/空密码）。全部按顺序进 RunConfig.passwords。
- 旧用法（无子命令、位置参数 .enc）保持可用，行为除多密码外不变；退出码/--json 约定不变。

### 7.7 图标与资产（Lead）

assets/icon.svg（源）+ gui/src-tauri/icons/（tauri 需要的 png/icns/ico）+ gui/public/icon.png（页头小图）；
新图标=蓝渐变圆角方、白色开盖纸箱+拉链、金色开锁挂锁（区别于参考的 7-Zip/WinRAR/Archive Utility）。

### 7.8 文档/脚本（docs-reader）

把 README、docs/{BUILD,WINDOWS_BUILD,VERIFICATION,OUTPUT-LAYOUT,MERGE-MAPPING,CORE_API}、
scripts/{build.sh,build-windows-portable.sh,build-windows.ps1,e2e-verify.sh,make-fixtures.sh,sync-binaries.mjs}、
.github/workflows/build.yml、NOTICE、binaries/README.md 更新到 Unparanoia 0.3.0：新名字/版本/产物名
（Unparanoia_0.3.0_aarch64.dmg、Unparanoia-0.3.0-win-x64-portable.zip、Unparanoia.exe）、密码管理页、
主题、多密码逐层尝试、图标。产物哈希留「待最终构建回填」，由 Lead 构建后统一替换；不要运行 GUI/dmg/Windows 构建。

## 8. v0.3.1 增补（2026-10-24，最新契约，覆盖前文冲突处）

版本 **0.3.1**（产品名仍为 Unparanoia）。用户报 4 项：①Windows 11 真机白屏 localhost 拒绝连接；
②分段压缩包识别/解压（含 .enc 分卷，真实样本 /Volumes/Carrot/Download/3138.7z.001.enc + .002.enc，
解密密码 123，7z 密码 anzixin）；③Windows 尽量发**单独 exe**、不依赖外部文件；④最终产物放桌面新建文件夹。

### 8.1 Windows 白屏根因与修复（app-reader；Lead 已定位）

根因：GUI 的 tauri 未启用 custom-protocol feature。Tauri 2.12 的 tauri-macros / tauri-codegen 在
cfg!(not(feature = "custom-protocol")) 时判定为 dev 构建、不内嵌 frontendDist，运行时加载 devUrl =
http://localhost:1420 → Windows exe 直接双击即 ERR_CONNECTION_REFUSED。macOS 走 pnpm tauri build
（CLI 自动加 feature）所以正常；Windows 走裸 cargo xwin build 没有该 feature。

修复（gui/src-tauri）：

```toml
[features]
default = []
custom-protocol = ["tauri/custom-protocol"]
```

Windows GUI 构建命令必须加 --features custom-protocol。验收证据：strings <exe> 命中内嵌前端中文串
（如「帕罗诺亚解包器」「本次追加密码」）；注意 localhost:1420 配置串在两种构建里都会出现，不能作为判据。

### 8.2 Windows 单 exe：内嵌 7-Zip（Lead 已建 crate，app-reader / core-reader 接入）

新 crate crates/embedded-7z（package unparanoia-embedded-7z，已加入根 workspace，Lead 维护）：

```rust
pub const SEVEN_ZIP_VERSION: &str = "25.01";
pub const LICENSE_TEXT: &str; // 7-Zip LGPL + unRAR 限制全文
pub fn ensure_7z() -> Result<Option<PathBuf>, Embedded7zError>;
```

Windows：用 include_bytes! 把 binaries/win/7z.exe + 7z.dll 打进调用方 exe，首次运行解压到
%LOCALAPPDATA%\\Unparanoia\\embedded-7z\\25.01-r1\\（UNPARANOIA_7Z_CACHE_DIR 可覆盖），返回 7z.exe 路径；
非 Windows：Ok(None)（无副作用）。7-Zip 许可证同时写入缓存目录。

接入要求：
- gui/src-tauri/Cargo.toml、crates/cli/Cargo.toml 加依赖 unparanoia-embedded-7z = { workspace = true }。
- GUI locate_sevenzip()：先 ensure_7z() 成功则 SevenZip::locate(Some(path))；失败只记录日志并回落
  SevenZip::locate(None)（不得让 GUI 起不来）。
- CLI build_sevenzip()：显式 --sevenzip 优先；否则先 ensure_7z() 得内嵌路径再 locate(Some(path))；
  失败回落 locate(None)。doctor 打印 7-Zip 来源（内嵌/外部/环境变量）与版本。
- CLI 新增 --licenses（或在 doctor 输出）打印 LICENSE_TEXT；GUI 设置页展示「内嵌 7-Zip 25.01（LGPL + unRAR 限制）」
  与许可证查看入口（可新增命令 seven_zip_license() -> String，前端 owner gui-frontend）。
- Windows 产物不再附带 7z.exe/7z.dll 侧车文件；macOS .app 继续由 bundle resources 带 binaries/mac/7zz。

### 8.3 分段压缩包（engine-dev，crates/unpack-core）

命名族：7z/zip 为 <stem>.7z.001、<stem>.zip.001…（首卷 = .001）；RAR 为 <stem>.part1.rar…。
族键 = 去掉卷号后的 <stem>.<ext>。只有第一卷是 7-Zip 入口，7-Zip 会自动读取同目录其它卷。

输入分组（在 run_batch / 队列前置做，公共 API 尽量不变）：
1. 普通分卷：一次输入含同族多个卷（或目录展开后）→ **合并为一个逻辑任务**，任务 path = 第一卷；其余卷不单独发
   TaskQueued。7z 直接解首卷即可。
2. SSEFE 分卷：<stem>.7z.001.enc + <stem>.7z.002.enc → 同一逻辑任务，按候选密码逐个解密
   （verify→decrypt）到任务 staging 子目录（例如 staging/volumes/<stem>/），**全部卷解密完成后**再交给 7z 解压；
   任一卷缺密码/密码错 → 整个任务 NeedsPassword，清理 staging、不落半成品。
3. 只给非首卷（如仅 .002.enc）→ Skipped，reason 明确「分卷压缩包：请添加第一卷（.001 / .part1.rar）」并给出期望首卷名；
   不报 Failed。若同族首卷已在本次输入中，则按组合并处理（用户拖任意一卷都能工作）。

检测：以第一卷代表整族返回 Detection：kind=Archive，container = "7z-volume"/"zip-volume"/"rar-volume"，
format = "7z"/"zip"/"rar"，notes 含「分卷压缩包（N 卷，合计 M B）」；total_entries/top_level/
total_uncompressed/encrypted_entries 用 7z list 结果。内部可新增 SplitFamily 结构，不改 FileKind。

解压与密码：主解压沿用多密码重试（v0.3.0 契约）——SSEFE 解密用密码库中的 123，7z 解压再命中密码库中的
anzixin，不需要用户手输。分卷解压失败/缺密码：清理 staging、不落半成品。

布局：archive_stem 必须 3138.7z.001 → 3138、x.part1.rar → x（不能得到 3138.7z / x.part1）。
嵌套：内层级联同样支持分卷族；内层解出多卷时先移到同一目录再尝试 7z。

真实样本（只读，不可修改源文件）：/Volumes/Carrot/Download/3138.7z.001.enc（209,779,415 B）+
3138.7z.002.enc（60,527,110 B）；解密后 3138.7z.001 209,715,200 B、3138.7z.002 60,508,410 B。
engine-dev 必须用 CLI 手动验证（可先 7zz l 用密码 anzixin 查看，再完整解压到 /tmp 验证后删除；大产物不进仓库）。

### 8.4 构建/产物与文档（docs-reader，Lead 最终构建）

- 版本 0.3.1。macOS：./scripts/build.sh → Unparanoia_0.3.1_aarch64.dmg + .app（Resources 仍带 7zz/license）。
- Windows：./scripts/build-windows-portable.sh 改为**单 exe 交付**：
  pnpm --dir gui build → cargo xwin build --release --target x86_64-pc-windows-msvc -p unparanoia --features custom-protocol
  → cargo xwin build --release --target x86_64-pc-windows-msvc -p unparanoia-cli →
  产物 target/windows-portable/Unparanoia-0.3.1-win-x64.exe（GUI 单文件）与
  unparanoia-cli-0.3.1-win-x64.exe（可选单文件）；不再复制 7z.exe/7z.dll；可附 SHA256SUMS.txt/README.txt；
  不要求 zip/NSIS/安装器。
- 桌面交付（Lead）：~/Desktop/Unparanoia-0.3.1/ 放 dmg、Windows GUI exe、CLI exe、SHA256SUMS.txt、README.txt
  （中文：mac 拖入 Applications；Windows 双击 exe；首次运行把内嵌 7-Zip 解压到 %LOCALAPPDATA%\\Unparanoia；
  WebView2 由 Win11 自带；分段压缩包用法；密码库明文保存说明；未签名提示）。
- 文档：README 功能表加「分段压缩包（含 .enc 分卷）」「Windows 单 exe（7-Zip 内嵌）」；docs/WINDOWS_BUILD.md 写
  内嵌机制/缓存路径/首次启动解压/无侧车文件；docs/VERIFICATION.md 记录 Windows 白屏根因（custom-protocol 缺失）
  与修复验证（strings 内嵌前端串证据），并如实写明「构建后仍需用户在 Windows 11 真机确认」；
  docs/{BUILD,OUTPUT-LAYOUT,MERGE-MAPPING}.md 更新分卷布局、版本、产物名。哈希留「待 Lead 最终构建回填」。
- scripts：make-fixtures.sh 增加分卷 fixtures（7zz a -v1m split.7z ... → split.7z.001/002，再用
  make_enc 生成 split.7z.001.enc/split.7z.002.enc，密码 123；e2e 断言：两卷一起 → 成功解出；
  只给 .002 → Skipped 且 reason 含「第一卷」；SSEFE 分卷组成功）。e2e-verify.sh 增加对应断言与单 exe 产物存在性。
- CI：Windows job 加 --features custom-protocol，产物改为单 exe（不再 NSIS/侧车 7z）。

### 8.5 本轮写作用域

Lead：crates/core/**、crates/embedded-7z/**、根 Cargo.toml/Cargo.lock、最终构建与桌面交付、docs/DEV-CONTRACT.md。
engine-dev：crates/unpack-core/**。app-reader：gui/src-tauri/**。core-reader：crates/cli/**。
docs-reader：docs/**（除 DEV-CONTRACT）、README.md、scripts/**、.github/**、binaries/README.md、NOTICE。
gui-frontend：仅在收到 Lead 通知后改 gui/src/**（许可证查看入口/分卷提示可选）。

---

## 9. v0.3.2 增补：跳过记录、分卷行归并、Windows 控制台窗口与诊断日志

### 9.0 背景与根因（Lead 已定位）

- 用户 m01610 反馈：把 3138.7z.001.enc + 3138.7z.002.enc 拖入 macOS GUI，进度条停在 50%、
  无输出，日志显示「成功 0 / 跳过 1」。根因 = `crates/unpack-core/src/engine.rs` 的
  processed 指纹跳过：engine-dev 调试真实样本时用**默认配置目录**跑 CLI，把两条 `/tmp/...`
  调试路径的成功记录写进用户真实的 `~/Library/Application Support/unparanoia/processed.json`
  （fingerprint `8b6d68e7...` 正是 3138.7z.001.enc）。v0.3.1 的 `skip_processed` 默认 true，
  因此真实文件被静默跳过；分卷族只产生 1 个任务，所以只有 001 行被标记、002 行留在「待处理」，
  进度按 2 个文件算停在 50%。
- 教训：默认开启「按指纹跳过」对用户不可见且易被污染；分卷族的多行状态必须一起收尾。
- Windows 端「不时弹出空白命令行窗口」根因：`crates/unpack-core/src/sevenzip.rs` 的 `run()`
  用 `Command::new(7z)` 启动控制台程序，Windows GUI 子系统下没有 `CREATE_NO_WINDOW` 标志，
  每次调用 7z（探测/密码校验/解压/版本）都会闪一个控制台窗口。

### 9.1 引擎（owner：engine-dev，作用域 `crates/unpack-core/**`）

1. `types.rs` `TaskOutcome` 新增 `pub related_paths: Vec<PathBuf>`（默认空；`TaskOutcome` 没有
   `rename_all`，序列化键就是 `related_paths`，前端按 snake_case 读取）。
2. `engine.rs::process_file`：`let split_family = split::discover(path);` 之后立即
   `out.related_paths = family.volumes.iter().map(|v| v.path.clone()).collect();`（含
   `missing_first()` 的 bail 分支；即 skipped/failed/success 都带全族卷路径，顺序按卷号）。
   非分卷任务保持空数组。
3. `types.rs` `RunConfig::default().skip_processed` 改为 `false`。
4. `settings.rs`：
   - `Settings` 新增 `pub settings_version: u32`（camelCase `settingsVersion`，`#[serde(default)]`
     → 旧文件缺字段得 0），`Default` 实现里 = 2，`save` 原样写 2。
   - `load_from` 反序列化后迁移：`if settings_version < 2 { skip_processed = false; settings_version = 2; }`。
     新增测试：旧 JSON `{"skipProcessed":true}` → false 且 version=2；新 JSON
     `{"settingsVersion":2,"skipProcessed":true}` → 保持 true；默认 skip_processed=false；
     `serde` camelCase 往返含 `settingsVersion`。
5. `engine.rs` 跳过文案改为：
   `已处理过（历史记录跳过）：如需重新解压请点「重新解压」或在设置中关闭「跳过已处理」`
   （保留「已处理过」前缀，便于 e2e/文档断言）。
6. `sevenzip.rs`：
   - Windows：`run()` 与 `version()` 构造 `Command` 后设置
     `creation_flags(0x08000000 /* CREATE_NO_WINDOW */)`（`#[cfg(windows)] use
     std::os::windows::process::CommandExt`），确保 GUI 不闪控制台窗口。
   - Windows spawn 失败且 `raw_os_error()` 为 5/32/225（拒绝访问/占用）时重试最多 3 次、
     间隔 150ms（`tokio::time::sleep`），仍失败才报 `UpError::SevenZipFailed`。
   - `SevenZipFailed` 的 message 必须包含：7z 可执行文件路径、OS 错误/退出码、stderr 末尾
     ≤1000 字符；附建议「可设置 UNPARANOIA_7Z 指向已安装 7-Zip」。检查现有 `SevenZipFailed`
     构造点，保证 GUI 的 `reason` 能看到这些信息。
7. processed 记录 gate（默认关闭时不留记录）：成功路径仅当 `cfg.skip_processed == true` 时
   才 `processed.insert`；`queue.rs` 在 `run_batch` 末尾仅当引擎 `cfg.skip_processed` 时
   `save_processed()`。测试：默认配置跑完 processed 记录为空；开启后写入并可被跳过。
8. 测试：更新 `settings`/`engine` 相关断言；新增 `related_paths` 断言（分卷族成功/缺首卷）；
   `cargo test -p unparanoia-unpack-core` 全绿、`cargo check -p unparanoia-unpack-core --all-targets`
   0 warning。

### 9.2 GUI 后端（owner：app-reader，作用域 `gui/src-tauri/**`）

1. `AppInfo` 新增 `logPath: String`、`logDir: String`（camelCase，`<configDir>/logs/`）。
2. 持久化日志：启动时 `create_dir_all(configDir/logs)`，追加写 `unparanoia.log`（>2MiB 时
   轮转为 `unparanoia.log.1`，只保留 1 份）。记录：启动信息、7-Zip 解析结果与来源、
   `start_batch` 参数摘要（**绝不写密码**）、每个 TaskEvent 的关键行、错误/panic、批次 summary。
3. 新增 `#[tauri::command] append_log(line: String) -> Result<(), String>`：前端 `pushLog` 调用，
   追加「时间戳 + line」。新增 `#[tauri::command] diagnostics() -> Diagnostics`（camelCase）：
   version、os/arch、configDir、settingsPath、sevenZip、sevenZipVersion、logPath、logDir、
   processedPath、processedCount、passwordCount（只给数量）、stagingPrefix、settingsJson；
   同时把诊断写一行日志。注册后命令总数 24。
4. `setup` 里 `Settings::load()` 之后尽力 `settings.save()`（把 v2 迁移持久化）；失败仅 stderr。
5. 事件名/事件结构/其余命令不变。`cargo check -p unparanoia --features custom-protocol --all-targets`
   0 warning。

### 9.3 前端（owner：gui-frontend，作用域 `gui/src/**`、`gui/index.html`）

1. 类型：`TaskEvent` 的 `task_finished.outcome` 增加 `related_paths: string[]`；`AppInfo` 增加
   `logPath`/`logDir`（后端未就绪时可选，做空值兜底）；`Settings` 增加 `settingsVersion?: number`。
2. `DEFAULT_SETTINGS.skipProcessed = false`；设置页该项文案改为
   「跳过已成功处理过的文件（默认关闭；开启后按 processed.json 指纹跳过）」，并保留「清除处理记录」。
3. `task_finished`：先按现有逻辑 patch 主行；再遍历 `outcome.related_paths` 中
   `!= outcome.path` 的路径，**仅当队列里已有该 key 的行**时 `patchByKey`：
   `phase:"done"`、`status` 同主行、`percent: 100`（success/skipped）、
   `reason: "已并入 " + basename(outcome.path) + "（分卷）"`；不得新建未知行。
4. `unparanoia://done`：对仍在 `queued`/`running` 且 key 属于本次 `batchKeys` 的行，置
   `phase:"done"`、`status:"skipped"`、`reason:"未处理：同一分卷任务已由首卷处理"`。
   若 `summary.success===0 && summary.skipped>0 && needs_password===0 && failed===0`：
   toast + 日志提示「文件因历史处理记录被跳过：可点「重新解压」，或在设置中清除处理记录」，
   并在队列顶部显示一条提示条，含按钮「清除处理记录」与「全部重新解压」
   （后者 `startPaths(全部 batchKeys, {skipProcessed:false})`）。
5. 日志页「复制/清空」旁新增「打开日志目录」→ `invoke("reveal_path",{path: info.logDir})`；
   设置页新增「诊断信息」区块：显示版本、7-Zip 路径/版本、配置目录、日志路径，按钮
   「复制诊断信息」（`invoke<Diagnostics>("diagnostics")` 后复制 JSON）、「打开日志目录」、
   「清除处理记录」。`pushLog` 内同时 `invoke("append_log",{line}).catch(()=>{})`（fire-and-forget）。
6. `pnpm --dir gui build` 通过；不使用 `any`/`innerHTML`/`dangerouslySetInnerHTML`；无新增运行时依赖。

### 9.4 CLI（owner：core-reader，作用域 `crates/cli/**`）

1. `RunFlags` 新增 `--skip-processed`（bool，opt-in）；保留 `--no-skip-processed`（hidden、
   兼容旧脚本，等价默认行为，不再报错）。`apply_run_flags`: `if flags.skip_processed {
   cfg.skip_processed = true; }`。
2. 更新 `--help`/模块文档/`print_help` 文案：默认不跳过；`--skip-processed` 才启用指纹跳过。
3. `run` 结束时只有在 `cfg.skip_processed` 时保存 processed（现有逻辑保持）；`doctor` 保留
   processed.json 路径并可打印记录数（已有则确认）。
4. `--json` 输出的 `outcome` 自动带 `related_paths`；无需特殊处理。
5. `cargo fmt --check`、`cargo check -p unparanoia-cli --all-targets` 0 warning；legacy 模式行为不变。

### 9.5 文档/脚本（owner：docs-reader，作用域 `docs/**`、`README.md`、`scripts/**`、`.github/**`；
   被 9.1–9.4 阻塞，Lead 通知后再动）

1. 版本 0.3.2；README「与 v0.3.1 的行为差异」增加：跳过已处理默认**关闭** + 首次升级会把旧的
   `skipProcessed:true` 迁移为 false（只一次）；历史记录跳过的提示/重新解压/清除处理记录；
   分卷族只产生一条任务、所有卷行统一标记；Windows 空白控制台窗口已修复（CREATE_NO_WINDOW）；
   新增日志文件与诊断信息位置。
2. `scripts/e2e-verify.sh` 增补：①默认（不带 `--skip-processed`）重复运行不跳过；
   ②`--skip-processed` 第二次 Skipped 且 reason 含「已处理过」；③分卷任务 `--json` 的
   outcome 含两个 `related_paths`；④清除 processed.json 后再跑成功。保持 91 项旧断言全过。
3. `docs/OUTPUT-LAYOUT.md` skip_processed 段落改为默认关闭；`docs/VERIFICATION.md`/`docs/BUILD.md`/
   `docs/WINDOWS_BUILD.md` 版本与日志/诊断/Windows 控制台修复说明，0.3.2 哈希位留「待 Lead 回填」。
4. 不改 `docs/DEV-CONTRACT.md`。

### 9.6 Lead

- 版本 0.3.2：根 `Cargo.toml` `[workspace.package].version`、`gui/package.json`、`gui/src-tauri/tauri.conf.json`。
- 清理用户真实配置目录里两条 `/tmp` 调试记录（`~/Library/Application Support/unparanoia/processed.json`）。
- 依次：`cargo test --workspace`、`pnpm --dir gui build`、`./scripts/build.sh`、
  `./scripts/build-windows-portable.sh`；真实样本手动复验（两卷一起 / 只给 .001 / 只给 .002）；
  GUI 冒烟与截图；回填文档哈希；中文 git commit；桌面 `~/Desktop/Unparanoia-0.3.2/`。
- 最终回复向用户索取 Windows 端日志（日志页 → 打开日志目录 → `unparanoia.log`）以定位 Windows 解压失败。
