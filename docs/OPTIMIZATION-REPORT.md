# Upzip v0.4.0 优化验收报告

- 项目：`upzip-desktop`（Upzip v0.4.0）
- 报告生成时间：2026-11-17（Asia/Shanghai）
- 验证机器：macOS 27.3 / arm64，Rust 1.100.0（Homebrew），Node v26.10.0，pnpm 11.27.0，内置 7-Zip 25.01（`binaries/mac/7zz`）
- 契约基线：`docs/FORMAT.md`、`docs/OUTPUT-LAYOUT.md`、`docs/CORE_API.md` 未改动；容器格式、错误语义、`skip_processed` 默认关闭、内层缺密码保留外层并 exit 0、`--layout smart` 规则均保持不变。

---

## 1. 变更摘要

本次共完成 10 项目标，覆盖 5 个 Rust crate 与 GUI 前后端：

| # | 目标 | 主要文件 |
|---|------|----------|
| 1 | 内层级联不再拆散 Office/JAR 文档容器（`force_documents` 可覆盖） | `crates/unpack-core/src/engine.rs` |
| 2 | `--no-strip-junk` / GUI「清理系统垃圾条目」真正关闭 7-Zip 垃圾过滤 | `crates/unpack-core/src/sevenzip.rs`、`engine.rs` |
| 3 | `ListResult::is_junk_path` 不再把尾随 `/`、空段误判为垃圾 | `crates/unpack-core/src/sevenzip.rs` |
| 4 | 跨目录同名分卷家族按父目录独立归组 | `crates/unpack-core/src/split.rs` |
| 5 | GUI「递归展开文件夹」接入 + 非递归扫描根目录第一层 | `gui/src-tauri/src/lib.rs`、`crates/unpack-core/src/queue.rs` |
| 6 | 消除重复 16 MiB 指纹计算；重 I/O 移入 `spawn_blocking`；雕刻可取消 | `crates/unpack-core/src/engine.rs`、`detect.rs` |
| 7 | Windows `free_space` 走 `GetDiskFreeSpaceExW` | `crates/unpack-core/src/util.rs` |
| 8 | GUI 日志去重、任务结束清理过期进度、前后端类型对齐 | `gui/src/App.tsx`、`gui/src/types.ts`、`gui/src/task-logic.ts` |
| 9 | 密码首尾空格不再被 trim；版本兜底改为 `package.json` | `gui/src/App.tsx`、`PasswordsTab.tsx`、`vite.config.ts` |
| 10 | 环境变量重复项、旧临时前缀/缓存名与过时注释清理 | CLI、GUI 后端、`embedded-7z`、`core`、`detect.rs` |

改动规模：18 个已跟踪文件修改（+941 / −152），新增 2 个前端文件（`gui/src/task-logic.ts`、`gui/tests/task-logic.test.ts`，共 76 行），无新增任何依赖。

公开 API 兼容性：

- `SevenZip::extract(...)` 签名与默认行为不变，内部委派到新方法。
- 新增（附加式）：`SevenZip::extract_with_options`、`detect::detect_with_fingerprint`、`detect::carve_to_temp_with_cancel`。
- CLI 退出码、默认参数、`RunConfig` 默认值均未改变。

---

## 2. 逐项修改对照表与实现说明

### 2.1 内层级联拦截 Office/JAR 文档容器

- **修改文件/符号**
  - `crates/unpack-core/src/engine.rs:26` `DOC_EXTS`
  - `crates/unpack-core/src/engine.rs:846-860`，`Engine::process_file` 的 `nested` 级联循环（`det_inner` 之后、`descend` 之前）
- **修改前**：`descend` 只按 `FileKind` 判断（`Archive | MaskedArchive | SfxExecutable | SsefeEncrypted`）。`.docx/.xlsx/.jar/.apk` 本质是 ZIP，`detect` 会给出 `Archive`（`office_like` 只作为提示），于是级联继续解包，把文档拆成 `word/document.xml` 等散件。
- **修改后**：在决定 `descend` 之前插入拦截——当 `!cfg.force_documents` 且（`probe_path` 扩展名小写命中 `DOC_EXTS`，或 `det_inner.office_like == true`）时，向 `det.notes` 写入「保留文档容器 …（启用 force_documents 可展开）」并 `break`；staging 中的文档原样保留，继续参与后续 layout/land。`cfg.force_documents == true` 时条件整体跳过，级联行为与之前一致。顶层文档拦截（`engine.rs:361-379`）为既有逻辑，未改动。
- **自动化测试**：`nested_document_container_is_preserved_unless_forced`（`crates/unpack-core/src/engine.rs:2558`）。构造内含 `word/document.xml` 的 `doc.docx` 与只含该文档的 `outer.zip`；`force=false` 断言输出根下 `doc.docx` 为文件且不存在 `word/`；`force=true` 断言 `word/document.xml` 存在且 `doc.docx` 不存在。

### 2.2 `--no-strip-junk` 真正生效

- **修改文件/符号**
  - `crates/unpack-core/src/sevenzip.rs:628` `extract`（保留原签名，固定 `strip_junk = true` 委派）
  - `crates/unpack-core/src/sevenzip.rs:646` `extract_with_options(..., strip_junk: bool, ...)`；参数构造见 `sevenzip.rs:660-672`
  - `crates/unpack-core/src/engine.rs:1489` `extract_once` 传入 `self.cfg.strip_junk`
  - `crates/unpack-core/src/engine.rs:786-797` 与 `:1165-1167` 的 `layout::strip_junk_tree` 均以 `if cfg.strip_junk` 包裹
- **修改前**：`SevenZip::extract` 无条件向 7-Zip 传 `-xr!__MACOSX`、`-xr!.DS_Store`、`-xr!._*`、`-xr!Thumbs.db`，`RunConfig::strip_junk` 只影响落地后的 `strip_junk_tree`，因此 `--no-strip-junk` 下压缩包里的垃圾条目仍被 7-Zip 丢弃；而级联结束后的第二次 `strip_junk_tree` 还会无视开关再清理一次。
- **修改后**：新增 `extract_with_options`，只有 `strip_junk == true` 才追加四个 `-xr!` 参数；`extract` 保持旧签名与旧语义（默认过滤）。引擎唯一解压入口 `extract_once` 传 `cfg.strip_junk`；两处落地/级联后的 `strip_junk_tree` 都遵循开关，`--no-strip-junk` 时既不解压过滤也不二次清理。
- **自动化测试**：`strip_junk_option_controls_extraction_exclusions`（`crates/unpack-core/src/sevenzip.rs:1047`）。用 7-Zip 生成含 `keep.txt`、`.DS_Store`、`__MACOSX/x` 的 ZIP，`strip_junk=false` 时断言 junk 文件存在，`strip_junk=true` 时断言被清理且 `keep.txt` 始终存在。

### 2.3 `is_junk_path` 空段误判

- **修改文件/符号**：`crates/unpack-core/src/sevenzip.rs:99` `ListResult::is_junk_path`
- **修改前**：`path.split('/').any(|c| c.is_empty() || JUNK_NAMES... || c.starts_with("._"))` 把空段也算作垃圾，`"docs/"` 产生尾随空段即被判为 junk，导致 `top_level(true)` 少算目录、影响 smart 折叠判断。
- **修改后**：`path.split('/').filter(|c| !c.is_empty()).any(...)`，只对非空组件匹配 `JUNK_NAMES` 或 `._` 前缀；`"empty_dir/"`、`"a//b.txt"`、`"./docs/"` 均保持为正常内容。
- **自动化测试**：`junk_path_ignores_empty_path_components`（`sevenzip.rs:1029`）。覆盖 `"docs/"`、`""`、`"./docs/"`、`"a//b.txt"`、`"empty_dir/"`（非 junk）与 `"__MACOSX/"`、`".DS_Store"`、`"docs/._x"`、`"a//Thumbs.db"`（junk）；既有 `junk_is_ignored_for_top_level` 继续保证 `top_level(true)` 计数。

### 2.4 跨目录同名分卷不再合并

- **修改文件/符号**
  - `crates/unpack-core/src/split.rs:371` `family_key`
  - `crates/unpack-core/src/split.rs:390` `family_dir`
  - `crates/unpack-core/src/split.rs:407` `lexical_normalize`
  - `crates/unpack-core/src/split.rs:330` `group_inputs`（未改签名，使用新 key）
- **修改前**：key 只有 `base|kind|encrypted`，`/dirA/a.7z.001` 与 `/dirB/a.7z.001` 被当作同一家族，批处理只保留一个，另一个被静默丢弃。
- **修改后**：key 追加规范化父目录。`family_dir` 优先 `canonicalize`（解析 `.`/`..`/符号链接）；路径不存在时，绝对路径做词法规范化，相对路径先拼 `current_dir` 再规范化（`ParentDir` 仅在前一段是普通组件时回退，否则保留 `..`），因此相对/等价写法仍能合并，同名不同目录各自独立成任务。
- **自动化测试**：`group_inputs_keeps_same_named_families_in_different_dirs_apart`（`split.rs:583`）断言 `/dirA/a.7z.001`、`/dirB/a.7z.001` 各自代表一项；`group_inputs_merges_equivalent_relative_parent_dirs`（`split.rs:603`）断言 `./relA/a.7z.002`、`relA/a.7z.001`、`relA/./a.7z.003` 合成一项。

### 2.5 GUI 递归开关与非递归扫描语义

- **修改文件/符号**
  - `gui/src-tauri/src/lib.rs:453-460` `expand_paths`：`let recursive = settings.recursive; ... expand_inputs(&inputs, recursive, exclude.as_deref())`
  - `gui/src-tauri/src/lib.rs:574` `start_batch`：`expand_inputs(&inputs, req.settings.recursive, output_root.as_deref())`
  - `crates/unpack-core/src/queue.rs:49` `collect`：`if !recursive && !is_root { return; }`
- **修改前**：GUI 两处硬编码 `true`，设置里的 `recursive` 完全不生效；`queue::collect` 对非递归直接 `return`，导致传入文件夹时（含 CLI `--no-recursive`）一个文件都收不到。
- **修改后**：GUI 读取持久化设置/请求设置；`collect` 仅在「非根目录且非递归」时停止，根目录在非递归下扫描第一层直属文件、不进入子目录。CLI `--no-recursive` 由「忽略文件夹」变为「只扫文件夹第一层」，与其文案「不递归子目录」一致，e2e 相关用例保持通过。
- **自动化测试**：`non_recursive_root_scan_collects_direct_files_only`（`queue.rs:238`）：根目录下 `top.zip` 与 `sub/inner.zip`，`expand_inputs(..., false, None)` 只返回 `top.zip`，`recursive=true` 返回 2 个。

### 2.6 指纹去重、阻塞 I/O 与雕刻取消

- **修改文件/符号**
  - `crates/unpack-core/src/engine.rs:246-269`：仅当 `cfg.skip_processed` 时预计算 `fp`；`detect::detect_with_fingerprint(..., Some(fp))`；检测后 `if cfg.skip_processed && fp.is_empty() { fp = det.fingerprint.clone(); }`
  - `crates/unpack-core/src/detect.rs:615` `probe_head_blocking(path, known)`，`:643` `detect`，`:649` `detect_with_fingerprint`
  - `crates/unpack-core/src/detect.rs:466` `carve_to_temp`（保留签名，委派空 token）与 `:473` `carve_to_temp_with_cancel`；缓存目录 `:480` 改为 `upzip-carve`
  - `crates/unpack-core/src/detect.rs:800-834` 雕刻走 `spawn_blocking`，`Err(UpError::Cancelled)` 不再被吞
  - `crates/unpack-core/src/engine.rs:1207-1238` 落地（`move_with_policy_landing` + 逐项取消检查）移入 `spawn_blocking`
- **修改前**：`process_file` 无条件先算一次 `fingerprint`（size + 文件名 + 头部 8 MiB SHA-256，`util.rs:29-40`），`detect` 内部再算一次；`detect`/`carve_to_temp`/`land` 的磁盘 I/O 在 Tokio 异步线程同步执行；`carve_to_temp` 拷贝数 GB 时不可取消；雕刻后 `sz.list(&tmp, ..., cancel)` 的 `Err(_)` 分支连 `Cancelled` 一起吞掉并继续其他偏移。
- **修改后**：
  - 默认 `skip_processed=false` 时引擎不再提前哈希，由检测阶段唯一一次计算；开启时引擎算一次并把结果作为 `known_fingerprint` 传入检测，全程仍只算一次。
  - 头部读取 + 指纹、尾部分析 + PE 探测 + stub 扫描分两个 `spawn_blocking` 阶段；雕刻在 blocking 池执行并可被 `CancelToken` 中断（拷贝前后与每 1 MiB 循环检查）。
  - 取消时删除 `.part-*` 并返回 `UpError::Cancelled`；雕刻后列表的 `Cancelled` 向上传播，不再被 `Err(_)` 吞掉。落地阶段同样在 blocking 池，取消码 `UpError::Cancelled` 映射为 `TaskStatus::Cancelled`。
- **自动化测试**
  - `detect_reuses_known_fingerprint`（`detect.rs:1191`）：`probe_head_blocking(None)` 的值与 `fingerprint` 一致；传入 `known` 时原样复用；`detect_with_fingerprint(..., Some(real))` 返回同一指纹。
  - `detect_returns_cancelled_before_touching_the_file`（`detect.rs:1219`）：预取消 + 不存在路径 → `UpError::Cancelled`，证明取消优先于 I/O 错误。
  - `carve_cancel_removes_partial_output`（`detect.rs:1231`）：取消的雕刻返回 `Cancelled`，`temp/upzip-carve` 下不产生目标文件、不残留 `.part-`。

### 2.7 Windows `free_space`

- **修改文件/符号**：`crates/unpack-core/src/util.rs`
  - `:98` `#[cfg(unix)] free_space`（新增祖先回溯，内部 `statvfs_free` 在 `:102`）
  - `:127` `#[cfg(windows)] free_space`：轻量 `extern "system" GetDiskFreeSpaceExW`（`:133`），无新 crate
  - `:144` `api_path`：UTF-16、去尾部分隔符（保留 `C:\` 与 `\\` 卷根）、长度 ≥248 时按 UNC → `\\?\UNC\`、盘符 → `\\?\` 加逐字前缀、补 NUL
  - `:183` `probe` 返回 `free_to_caller`；主函数自 `path` 向上回溯到首个可解析祖先
  - `:209` 兜底改为 `#[cfg(not(any(unix, windows)))]`
- **修改前**：Windows 无实现，`free_space` 恒为 `None`，因此 Windows 上「磁盘空间不足」预检查直接跳过；Unix 仅在目标目录已存在时可用。
- **修改后**：Windows 通过 `GetDiskFreeSpaceExW` 返回可用字节；不存在的输出目录自动回溯到存在的祖先/卷根；UNC、长路径、盘符根均有处理。Unix 同样支持未来子目录。
- **自动化测试**：`free_space_resolves_existing_dirs_and_future_children`（`util.rs:438`）：临时目录与不存在的嵌套子路径均返回 `Some(>0)`（unix/windows 生效，其他平台断言 `None`）。
- **限制**：本机没有 Windows 目标，未做编译或实机验证（见第 5 节）。

### 2.8 GUI 日志去重、完成态清理与类型对齐

- **修改文件/符号**
  - `gui/src/App.tsx:78-84` `pushLog(line, persist = true)`，仅 `persist` 为真时 `invoke("append_log")`
  - `gui/src/App.tsx:647` `const logEvent = (line) => pushLog(line, false)`，用于 `task_detected` / `task_password_attempt` / `task_password_missing` / `task_finished` / `log` 等后端事件驱动的展示日志
  - `gui/src/App.tsx:741-743`、`:753-755`：完成/失败/取消时 `percent: finishedPercent(...)` 且展开 `...FINISHED_TASK_RESET`（`line: null`、`stage: undefined`、`stageDetail: null`），`relatedPatch` 同步
  - `gui/src/task-logic.ts:8` `FINISHED_TASK_RESET`，`:15` `finishedPercent`
  - `gui/src/types.ts:71` `task_progress.percent: number | null`；`gui/src/types.ts:178` `tryEmptyPassword: false`
- **修改前**：后端 `run_batch_thread` 已把全部 `TaskEvent` 写入 `upzip.log`，前端又对每个事件 `invoke("append_log")`，日志重复落盘；`task_finished` 只更新 `percent`，旧 `line/stage/stageDetail` 残留；`percent` 类型不接受 `null`；GUI 默认 `tryEmptyPassword: true` 与 Rust `RunConfig::default()`（false）不一致。
- **修改后**：后端事件只更新 UI，不再写盘；用户主动操作（开始批次、添加密码等）仍 `persist=true`；完成态统一清理三个进度字段（含分卷 relatedPatch）；`percent` 改为可空，`TaskRow` 既有 `!= null` 判断保证不渲染 `null%`；默认值与核心一致。
- **自动化测试**：`gui/tests/task-logic.test.ts`（`pnpm --dir gui test:frontend`，5 项）断言完成态重置字段、成功/跳过为 100、失败/取消/需密码/`null` 为 `null`、`DEFAULT_SETTINGS.tryEmptyPassword === false`；`tsc` 保证 `number | null` 的类型贯通与空值兼容。

### 2.9 GUI 密码空格与版本兜底

- **修改文件/符号**
  - `gui/src/task-logic.ts:21` `isPasswordEmpty`（只判 `raw.length === 0`），`:26` `passwordToSubmit`（空→`null`，否则原样）
  - `gui/src/App.tsx:208` 添加密码判空、`:310` 本次运行密码、`:318` 记住密码条件
  - `gui/src/components/PasswordsTab.tsx:68` 按钮禁用条件
  - `gui/src/App.tsx:976` `displayVersion(info, __APP_VERSION__)`；`gui/src/task-logic.ts:31` `displayVersion`
  - `gui/vite.config.ts:5,15` 从 `package.json` 注入 `__APP_VERSION__`；`gui/src/vite-env.d.ts:4` 类型声明；`gui/tsconfig.node.json` 开启 `resolveJsonModule`
- **修改前**：`addPassword` 与 `startPaths` 对密码 `.trim()`，带首尾空格的真实密码被改写；`App.tsx` 兜底版本硬编码 `"0.3.3"`，与 0.4.0 不符。
- **修改后**：密码仅判空、原样保存/提交，与 `upzip-core::config::add_password_to`（只判 `is_empty`）及 CLI 一致；版本兜底改为构建期从 `gui/package.json` 注入，避免再次漂移。
- **自动化测试**：`gui/tests/task-logic.test.ts` 断言 `"  secret  "` 非空且原样提交、`""` 为空且提交 `null`、`displayVersion(null, "0.4.0") === "0.4.0"`；`pnpm --dir gui build`（`tsc && vite build`）通过。

### 2.10 环境变量、旧前缀与过时注释

- **修改文件/符号**
  - `crates/cli/src/main.rs:335-346` `env_password()`：`["UPZIP_PASSWORD", "UNPARANOIA_PASSWORD", "PARANOIA_ENC_PASSWORD"]`
  - `crates/cli/src/main.rs:489-497` `env_sevenzip_path()`：`["UPZIP_7Z", "UNPARANOIA_7Z"]`；帮助文本 `:123-125` 同步
  - `gui/src-tauri/src/lib.rs:133`、`:346`：`["UPZIP_7Z", "UNPARANOIA_7Z"]`，注释同步
  - `crates/embedded-7z/src/lib.rs:63-72` `cache_dir()`：`UPZIP_7Z_CACHE_DIR` 优先，回退 `UNPARANOIA_7Z_CACHE_DIR`
  - `crates/unpack-core/src/detect.rs:480`：`paranoia-carve` → `upzip-carve`
  - `crates/core/src/zip_extract.rs:80,166`：`.paranoia-enc-extract-` / `.paranoia-enc-unwrap-` → `.upzip-enc-*`；`crates/core/src/decrypt.rs:376` 残留断言改 `.upzip-enc-`
  - 过时注释：`gui/src-tauri/src/lib.rs:1-14`（crate 名、事件名）、`:44-46`（日志轮转名）、`:223`、`:640`（`upzip://done`）、`:992`（`.upzip-tmp-*`）
- **修改前**：`env_password`/`env_sevenzip_path`/`cache_dir` 的兼容数组写成了重复的新名字，`UNPARANOIA_*` 回退实际不存在；仍使用 `paranoia-carve` 与 `.paranoia-enc-*` 临时前缀；GUI 后端注释仍描述旧 crate/事件/临时目录名。
- **修改后**：新名优先、旧名回退的三级/两级数组恢复；临时文件与缓存目录统一 `upzip-*`；与实际代码矛盾的注释更新为 `upzip-core`、`upzip-unpack-core`、`upzip://event`、`upzip://done`、`upzip.log`、`.upzip-tmp-*`。`detect.rs:600` 的 `format: "paranoia-enc-v4"` 是容器格式名（契约值），保持不变。
- **自动化测试**
  - `env_password_prefers_new_name_and_falls_back`（`crates/cli/src/main.rs:1899`）：三级优先级，空值不遮蔽低优先级。
  - `env_sevenzip_path_prefers_new_name_and_falls_back`（`main.rs:1915`）：旧名回退、新名优先（用临时真实文件）。
  - `cache_dir_env_override_prefers_new_name`（`crates/embedded-7z/src/lib.rs:179`）：legacy 回退、new 优先，结束时恢复环境。
  - `decrypt` 既有测试改为断言成功解包后目录内不残留 `.upzip-enc-`。

---

## 3. 验证执行记录

以下均为最终修改完成后在本机实际执行的结果（未做任何事后修饰）。

环境：macOS 27.3 arm64；`rustc 1.100.0 (0ef1bc386 2026-11-09)`；`cargo 1.100.0`；Node `v26.10.0`；pnpm `11.27.0`；内置 7-Zip `25.01 (arm64)`。

| 命令 | 退出码 | 实际结果摘要 |
|------|--------|--------------|
| `pnpm --dir gui sync:binaries` | 0 | `ok gui/src-tauri/binaries/mac/7zz`、`ok .../win/7z.exe`、`ok .../win/7z.dll`、`ok .../7z-LICENSE.txt`；`0 updated, 0 missing source(s)` |
| `cargo test --workspace` | 0 | **128 passed / 0 failed / 2 ignored**；编译 0 warning（`grep -c '^warning'` = 0） |
| `pnpm --dir gui test:frontend` | 0 | `tests 5 / pass 5 / fail 0`（`task-logic.test.ts`） |
| `pnpm --dir gui build` | 0 | `tsc` 0 error；`vite v8.3.1`，33 modules；`dist/index.html 0.52 kB (gzip 0.35)`、`dist/assets/index-*.css 13.99 kB (gzip 3.37)`、`dist/assets/index-*.js 285.58 kB (gzip 86.93)` |
| `./scripts/e2e-verify.sh` | 0 | **104 项检查全部通过** + `ALL E2E CHECKS PASSED`；覆盖 14 个检查段（准备、基础命令、detect/inspect、SSEFE、嵌套级联、普通/加密/前缀 ZIP、口令失败、多层密码、JSONL、dry-run/conflict/layout、config/cleanup-staging、分卷、`--skip-processed`、退出码 2） |
| `cargo check -p upzip -p upzip-cli` | 0 | `Finished dev profile`，0 warning |
| `cargo check --workspace --all-targets` | 0 | `Finished dev profile`，0 warning（`grep -c '^warning'` = 0） |

`cargo test --workspace` 分项：

- `upzip_lib`：2 passed
- `upzip`（Tauri bin）：0
- `upzip-cli`：2 passed（新增 2 个 env 测试）
- `upzip_core` lib：44 passed；`tests/oracle_vectors.rs`：1 passed；`tests/real_sample.rs`：0 passed / **2 ignored**
- `upzip-embedded-7z`：4 passed
- `upzip-unpack-core`：75 passed
- doc-tests：4 个 crate 均为 0

**ignored 原因**：`crates/core/tests/real_sample.rs` 依赖用户自备真实样本（`PARANOIA_SAMPLE_ENC` / `PARANOIA_SAMPLE_SHA256` / `PARANOIA_SAMPLE_PASSWORD`），样本不入库，故默认 `#[ignore]`，需显式 `--ignored` 运行。

Windows 交叉编译检查（未执行成功，见第 5 节）：

- `cargo xwin --version` → `error: no such command: 'xwin'`
- `rustup target list --installed` → 仅 `aarch64-apple-darwin`

代码质量补充（诚实记录）：

- `cargo clippy --workspace --all-targets` 仍报告基线既有告警，例如 `field_reassign_with_default`（既有测试逐字段赋值 `RunConfig`）、`derive_partial_eq_without_eq` / `impl can be derived`（`types.rs`、`core/output.rs`）、`too_many_arguments`（`engine.rs:1274/1506/1580`，既有）、`op_ref`（`detect.rs:41/251`，既有）、`manual_div_ceil`（`split.rs:455`，既有）、`single_char_add_str`（`sevenzip.rs:765`，既有）、`len_zero`（`core/zip_extract.rs:30`，既有）。本次新增代码中引入的 `op_ref`（`is_mp4`）与测试内 `field_reassign_with_default` 已修复；其余告警均落在未改动的既有行。
- `cargo fmt --all -- --check` 在基线即不干净（如 `crates/core/examples/make_enc.rs`、`crates/core/src/batch.rs`），为避免无关 diff 未做全仓库格式化；新增代码沿用周围风格。

---

## 4. 验收核对清单

### 4.1 具体优化任务 1~10

- [x] **1 文档容器级联拦截**：`engine.rs:846-860`；测试 `nested_document_container_is_preserved_unless_forced`（`engine.rs:2558`）
- [x] **2 `strip_junk` 生效**：`sevenzip.rs:646` + `engine.rs:1489`；落地/级联清理按开关（`engine.rs:786,1165`）；测试 `strip_junk_option_controls_extraction_exclusions`（`sevenzip.rs:1047`）
- [x] **3 `is_junk_path` 空段**：`sevenzip.rs:99`；测试 `junk_path_ignores_empty_path_components`（`sevenzip.rs:1029`）
- [x] **4 分卷父目录 key**：`split.rs:371/390/407`；测试 `split.rs:583`、`split.rs:603`
- [x] **5 GUI 递归 + 非递归根扫描**：`gui/src-tauri/src/lib.rs:453-460,574`；`queue.rs:49`；测试 `queue.rs:238`
- [x] **6 指纹去重 / blocking / 取消**：`engine.rs:246-269,1207-1238`；`detect.rs:615,643,649,466,473,800-834`；测试 `detect.rs:1191/1219/1231`
- [x] **7 Windows `free_space`**：`util.rs:127-205`；测试 `util.rs:438`（跨平台断言；Windows 实机未验证）
- [x] **8 GUI 日志/完成态/类型**：`App.tsx:78-84,647,741-743,753-755`；`types.ts:71,178`；测试 `gui/tests/task-logic.test.ts`（5 项）
- [x] **9 密码空格 + 版本**：`App.tsx:208,310,318,976`；`PasswordsTab.tsx:68`；`task-logic.ts:21,26,31`；`vite.config.ts:5,15`；同一前端测试覆盖
- [x] **10 环境变量/前缀/注释**：`cli/main.rs:335,489,123-125`；`gui/src-tauri/src/lib.rs:133,346,1-14,44-46,223,640,992`；`embedded-7z/lib.rs:63-72`；`detect.rs:480`；`zip_extract.rs:80,166`；`decrypt.rs:376`；测试 `cli:1899,1915`、`embedded-7z:179`

### 4.2 工程与验收要求

- [x] 无重复逻辑/无新依赖/无编译警告：`cargo check --workspace --all-targets` 与 `cargo test --workspace` 均 0 warning；`tsc` 0 error
- [x] `pnpm --dir gui sync:binaries` 已执行（0 updated / 0 missing）
- [x] `cargo test --workspace` 全部通过（128 passed / 0 failed / 2 ignored，原因见 §3）
- [x] `./scripts/e2e-verify.sh` 输出 `ALL E2E CHECKS PASSED`（104 项）
- [x] `pnpm --dir gui build` 成功（tsc + vite，0 error）
- [x] `cargo check -p upzip` 与 `cargo check -p upzip-cli` 成功（0 warning）
- [ ] `cargo xwin check --target x86_64-pc-windows-msvc`：**未验证**（本机无 `cargo-xwin`、无 Windows target）
- [x] 未改动无关功能/注释；`docs/FORMAT.md`、`OUTPUT-LAYOUT.md`、`CORE_API.md` 未改；公开 API 仅新增
- [x] 未验证项已如实列出（第 5 节）

---

## 5. 未验证项与已知边界（诚实声明）

以下场景**没有**在本机实际执行，不能把「编译通过 / 单元测试通过」等同于真机通过：

1. **Windows 真机与 `GetDiskFreeSpaceExW`**：本机是 macOS arm64，`cargo xwin` 未安装，`rustup` 只有 `aarch64-apple-darwin`，因此 Windows 代码未编译、未运行。`free_space` 的盘符根、UNC（`\\server\share`）、`\\?\` 长路径、不存在的输出目录回溯等分支仅有代码审查与 Unix 对应测试，**Windows 实机调用结果未验证**。
2. **Windows 交叉编译**：`cargo xwin check --target x86_64-pc-windows-msvc`（`-p upzip --features custom-protocol` 与 `-p upzip-cli`）**未执行**，原因同上。故 Windows 编译零警告的结论不成立（未测）。
3. **GUI 人工交互烟测**：未启动真实 Tauri 窗口做点击、拖拽、原生文件对话框、密码输入（含首尾空格）、设置切换、日志文件轮转的人工验证；仅完成 `tsc`/`vite build`、5 个纯函数单测与代码审查。Tauri 后端 RUST 侧只有既有 2 个单测通过。
4. **GUI 日志去重的运行期验证**：`pushLog(..., persist=false)` 的落盘去重通过代码审查与类型检查，未在运行中的 GUI 里对比 `upzip.log` 行数；无法排除 Tauri 命令层其他写日志路径。
5. **`--no-strip-junk` 端到端**：7-Zip 参数层有单元测试（macOS 7zz）；「GUI 关闭开关 → 真实输出目录保留 `.DS_Store`/`__MACOSX`」未做 e2e。
6. **文档容器真实性**：测试用合成 `doc.docx`（ZIP + `word/document.xml`），未用真实 Microsoft Office/JAR/APK 文件；真实文档的 `office_like` 启发式未覆盖。
7. **跨平台**：e2e 仅在 macOS arm64 + 内置 7-Zip 25.01 上运行；Linux、Windows 的 e2e 未跑。忽略的 2 个 `real_sample` 用例需用户自备真实加密样本，未运行。
8. **Windows 内嵌 7-Zip**：`cache_dir()` 的环境变量优先级有单测，但 Windows 上实际解压/缓存目录创建未验证。
9. **格式化与 clippy 基线**：仓库在本次改动前就不是 `cargo fmt` 干净的，clippy 也存在历史告警；本次未做全仓库格式化，也未清零既有 clippy 告警（仅确保编译/测试 0 warning，且不新增同类告警）。
10. **性能量化**：任务 6 消除了重复 16 MiB SHA-256 与阻塞调用，但未做前后基准测量（无 benchmark 基线），因此不声称具体耗时提升数字。

---

## 附：主要改动文件清单

| 文件 | 关联任务 |
|------|----------|
| `crates/unpack-core/src/engine.rs` | 1、2、6 |
| `crates/unpack-core/src/detect.rs` | 6、10 |
| `crates/unpack-core/src/sevenzip.rs` | 2、3 |
| `crates/unpack-core/src/split.rs` | 4 |
| `crates/unpack-core/src/queue.rs` | 5 |
| `crates/unpack-core/src/util.rs` | 7 |
| `crates/cli/src/main.rs` | 10 |
| `crates/embedded-7z/src/lib.rs` | 10 |
| `crates/core/src/zip_extract.rs`、`crates/core/src/decrypt.rs` | 10 |
| `gui/src-tauri/src/lib.rs` | 5、10 |
| `gui/src/App.tsx`、`gui/src/types.ts`、`gui/src/components/PasswordsTab.tsx` | 8、9 |
| `gui/src/task-logic.ts`、`gui/tests/task-logic.test.ts`（新增） | 8、9 |
| `gui/vite.config.ts`、`gui/vite-env.d.ts`、`gui/tsconfig.node.json`、`gui/package.json` | 9（版本注入与测试脚本） |

相关文档：[OUTPUT-LAYOUT.md](./OUTPUT-LAYOUT.md)、[CORE_API.md](./CORE_API.md)、[FORMAT.md](./FORMAT.md)、[VERIFICATION.md](./VERIFICATION.md)。
