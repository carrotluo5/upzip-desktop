# 输出布局与处理流程（OUTPUT-LAYOUT）

本文描述 v0.3.2（Unparanoia）解包引擎**实际写入磁盘的行为**：目录怎么决定、内层怎么落盘、
分段压缩包怎么聚合、冲突怎么办、临时与残留如何管理、口令怎么重试、失败/跳过的判定。
所有规则以代码为准，位置指向 `crates/unpack-core/src/`（2026-10-24 v0.3.2 工作树；
函数名比行号更稳）。

## 1. 一次处理的任务阶段

每个输入文件在队列里产生一个 Task，阶段字符串（`TaskStage.stage`）依次为：

| stage | 含义 | 代码 |
|---|---|---|
| `detect` | 魔数 / EOCD / PE overlay / SSEFE 头扫描，判定 FileKind | `engine.rs` detect 阶段（`TaskStage{stage:"detect"}`） |
| `password` | 压缩包口令候选尝试（SSEFE 的候选尝试发生在 decrypt 阶段） | `find_7z_password` / `extract_with_password_retry` |
| `decrypt` | SSEFE v4 解密到临时 ZIP（含 checkcode 校验/MAC/BLAKE3） | `decrypt_ssefe_with_retry` |
| `extract` | 7-Zip `x` 解到 staging 临时目录 | `extract_once` |
| `nested` | 级联：对内层压缩包递归（深度 `max_depth` 默认 4，硬上限 8） | `engine.rs` nested 阶段 |
| `land` | 把 staging 结果按 layout/conflict 策略搬到最终目录 | `engine.rs` land 阶段 |
| `execute_sfx` | 可选的自解压执行回退（默认关闭，且非 Windows 不启用） | `engine.rs` execute_sfx 阶段 |

进度通过 `TaskProgress{percent, line}` 推送；7-Zip 的 stdout 行会原样进入 `line`
（可能带 `\b` 退格控制符，属于 7-Zip 输出特性，UI/CLI 只做展示）。

> v0.3.1 起，**同一分卷家族**的多个输入会先聚合为一个逻辑任务（`path`=首卷），
> 其余卷不单独建任务；详见 §2.1。

### 1.1 口令候选池与逐层重试（v0.3.0）

候选池由 `password_candidates(cfg, det)` 在**每一层**独立构建（外层和内层互不继承"已成功"的口令）：

1. `RunConfig.passwords`（source = `config`）：CLI 的 `-p`（可重复，按给定顺序）→
   环境变量 `UNPARANOIA_PASSWORD`（兼容旧名 `PARANOIA_ENC_PASSWORD`）→ 密码库
   `config.json` 的全部条目；GUI 用请求里的密码 + 密码库。
2. 归档注释里的提示（source = `hint`）。
3. 空口令（source = `empty`，仅当 `--try-empty-password`/设置开启）。

空字符串候选会被丢弃（空口令只在第 3 步显式加入），并按口令去重；`tried` 是
**1 基**的池内序号。尝试过程通过 `TaskPasswordAttempt{id, source, ok, tried}` 事件上报。

- 压缩包：先用 `--test` 单个条目快速验证（`find_7z_password`）；若正式解压时
  7-Zip 仍报"需要密码"（`UpError::PasswordRequired`），清空 staging 后用池中
  **剩余候选**重试（`extract_with_password_retry`）；其他错误不重试。
- SSEFE：先 `ssefe::verify_password` 快速校验，再正式解密；只有
  `SsefeError::WrongPassword` 才用剩余候选重试（`decrypt_ssefe_with_retry`），
  完整性/MAC 错误直接失败。
- 序号 >1 的候选成功时，`TaskOutcome.warnings` 追加 `使用候选密码 #N 成功`。
- 候选用尽：发 `TaskPasswordMissing`，状态 `NeedsPassword`，**不写任何输出**，CLI exit 1；
  GUI 显示"需要密码"可补口令重试。级联中内层缺密码时，已解出的外层文件保留并给出 warning。

> 事件里的 source 标签对用户口令统一是 `config`（CLI 的 `-p`/环境变量/密码库都先并入
> `RunConfig.passwords`），不要把它误解为"只用了配置文件"。

## 2. 最终目录：layout 三种策略

`layout::plan_dest(layout, archive, out_root, top_count)` 的规则：

- `parent` = 用户 `-o/--output-dir`；未给则用**输入文件所在目录**，再不行用 `.`。
- `foldered`：
  - `--layout direct` → **false**，内层内容直接摊在 `parent` 下；
  - `--layout folder` → **true**，强制 `parent/<stem>/`；
  - `--layout smart`（默认）→ `top_count > 1` 时为 **true**，否则直接摊开。
- 文件夹名 `<stem>` = 归档文件名去掉**一个**扩展名后再做 `sanitize_component`
  （非法字符替换、去首尾点/空格）：`sample.enc` → `sample`；`normal.zip` → `normal`。
  **分卷**先去卷号后缀再去一个扩展名：`3138.7z.001` → `3138`、`3138.7z.001.enc` → `3138`、
  `x.part1.rar` → `x`（`split.rs` 的 `archive_stem` 规则）。
- `top_count` 是解压后**顶层条目数**（不含垃圾条目，见 §5）。

实测（`testdata/fixtures`，2026-10-24）：

```text
normal.zip  (hello.txt + docs/)   -> out/normal/hello.txt, out/normal/docs/readme.txt   # smart: 2 个顶层
sample.enc  (内层只有 hello.txt)   -> out/hello.txt                                     # smart: 1 个顶层
sample.enc  --layout folder        -> out/sample/hello.txt
nested.enc  (级联后最内层 hello)   -> out/hello.txt
```

注意：`--layout smart` 的 `top_count` 按**最外层解包结果**计算；级联展开发生在
staging 内部，不会额外多包一层。

## 2.1 分段压缩包（v0.3.1）

卷名识别（`crates/unpack-core/src/split.rs`）：

| 家族 | 首卷 | 其余卷 | `container` |
|---|---|---|---|
| 7-Zip 分卷 | `<stem>.7z.001` | `.002`、`.003`… | `7z-volume` |
| ZIP 分卷 | `<stem>.zip.001` | `.002`… | `zip-volume` |
| WinRAR 分卷 | `<stem>.part1.rar` | `.part2.rar`… | `rar-volume` |
| SSEFE 加密分卷 | `<stem>.7z.001.enc` | `.002.enc`… | 解密后同上 |

规则：

- **聚合**：同目录/同批输入中同家族的卷合并为**一个对话框任务**，7-Zip 只从首卷进入；
  用户把 `.001` 与 `.002` 一起拖入不会产生两个任务，也不会重复解压。只给首卷时，
  同目录其余卷会被自动发现。**v0.3.2** 起 GUI 队列中同族卷行归并到首卷任务
  （`TaskOutcome.related_paths` 给出该族全部卷路径，GUI 据此合行并识别“全跳过”）。
- **布局**：`3138.7z.001` + `.002` → 输出 `3138/`（见 §2 的 stem 规则）。
- **只给非首卷**（例如只有 `.002`、没有 `.001`）：状态 `Skipped`（CLI exit 0），
  原因文案「分卷压缩包：请添加第一卷（.001 / .part1.rar）；期望首卷：<名字>」。
- **缺后续卷**（有首卷、卷数不全）：状态 `Failed`（整批 exit 1），
  原因文案「分卷压缩包缺少后续卷（找到 N/M 卷）」，**不写任何半成品输出**。
- **SSEFE 分卷**：每卷是独立的 Paranoia v4 容器，逐卷用同一候选池解密到
  staging 的 `volumes/<stem>/` 目录，再按 7-Zip 分卷读取；任一卷缺密码/解密失败 →
  `NeedsPassword`（或明确失败），清 staging，不产生半成品。
- **嵌套**：内层压缩包如果是分卷，同样按上述规则级联（§7），深度计入 `max_depth`。
- `detect` 对首卷报告：`kind=archive`、`container=7z-volume|zip-volume|rar-volume`、
  `format=7z|zip|rar`，`notes` 含「分卷压缩包（N 卷，合计 M B）」；`.enc` 首卷在解密前
  显示为 `ssefe_encrypted`，解密后 `TaskDetected` 变为分卷容器并附
  「已用 Paranoia v4 明文口令解密分卷（N 卷）」。

实测（`testdata/fixtures`，v0.3.1 e2e）：

```text
split.7z.001 (+同目录 002-004)                -> out/split/{hello.txt,blob.bin}   # 1 个任务
split.7z.001.enc + split.7z.002.enc (+同目录)  -> out/split/...                    # 聚合 + 候选 #2
只给 split.7z.002.enc（无 001）               -> Skipped，提示补第一卷，零产出
truncated.7z.001.enc（只有首卷）              -> Failed「缺少后续卷」，零半成品
```

## 3. 冲突策略（落地时）

`--conflict rename|skip|overwrite`（默认 `rename`；旧值 `error` 兼容映射为 skip）。
规则在 `layout::move_with_policy_landing`（目录）与文件分支：

- **文件**：
  - `rename`：目标目录已有同名 → 改成 `<stem> (<n>).<ext>`（n=1..10000，沿用旧版 0.1.x 的
    `hello (1).txt` 风格）；无扩展名则 `<stem> (<n>)`；n 用尽后退化为 `<stem>_copy`。
    实测：再次解压 `sample.enc` 得到 `hello (1).txt`（`layout.rs:104-126`）。
  - `skip`：不覆盖，删除本次 staging 中的该文件，计入 `skipped`。
  - `overwrite`：删除旧文件再搬入。
- **目录**（顶层目录与 `dest_root` 同名时）：
  - `rename`：整个目录搬到唯一兄弟名 `<name> (<n>)`，**绝不与旧目录交叉合并**；
  - `skip`/`overwrite`：进入逐条目合并。已存在的同名目录**只合并、不整目录删除**；
    子项冲突再按文件规则处理。`overwrite` 只删同名文件，不会清空用户已有目录。

## 4. 临时目录、原子落地与残留

- staging 目录 = `plan.parent` 下 `.unparanoia-tmp-<uuid>`（`TMP_PREFIX` 常量
  `crates/unpack-core/src/lib.rs`；创建见 `engine.rs:446-447`、`:846-847`）。
- 7-Zip 先解到 staging，`land` 阶段再用 `util::move_path` 落地：
  `fs::rename` 优先，跨卷时 `copy_recursive + remove`（`util.rs:144-152`）。
  因此同一文件系统内落地是原子的；跨卷是"复制后删除"，中途失败可能留下半成品
  目标文件（staging 仍在，可重试）。
- 正常结束：staging 由 `TempGuard` 兜底删除，明文临时 ZIP 的 guard **无条件**删除
  （即使失败也不保留明文）。
- 失败时：默认**清 staging**；若 `--keep-broken`，staging 保留以便人工检查
  （`engine.rs:113-130` 的 `bail!` 分支）。CLI 输出路径会指向 staging。
- 崩溃/断电残留：`.unparanoia-tmp-*` 目录/文件留在输出位置，可用
  `unparanoia-cli cleanup-staging --root DIR` 清理（GUI 设置页同功能）。
  该命令只删前缀匹配项，不动其他文件。
- 磁盘空间：解压前用 `total_uncompressed × min_free_ratio`（默认 1.1）与
  `plan.parent` 的可用空间比较，不足则提前失败（`engine.rs:429-443`）；
  未声明大小的归档跳过该检查。

## 5. 垃圾条目与清洗

- 默认 `strip_junk` 开启：落地后递归删除 `._*`、`__MACOSX/`、`.DS_Store`、
  `.Spotlight-V100/`、`.Trashes/`、`.fseventsd/`、`Thumbs.db`、`desktop.ini`
  （`layout::strip_junk_tree` / `is_junk_name`）。`--no-strip-junk` 关闭。
- 计算 `top_count` 时垃圾条目已被排除，所以"只有 `__MACOSX` + 一个真文件"的包
  走 smart 不会多套一层。

## 6. 跳过与重试

- **`skip_processed` 自 v0.3.2 起默认关闭**（旧配置一次性迁移：`settings.json` 缺
  `settingsVersion`/<2 时写入 `settingsVersion=2` 且 `skip_processed=false`）。
  默认每次都会真正重新解压，不生成 `processed.json`、不写处理记录。
- 只有显式开启（CLI `--skip-processed`，或 GUI 设置页开关）后才有跳过行为：
  成功处理过的输入会以 fingerprint（size + 小写文件名 + 头部 8 MiB 的 SHA-256，
  `util.rs:29-40`）记进 `processed.json`（配置目录，默认
  `~/Library/Application Support/unparanoia/` / `%APPDATA%\unparanoia\`）。
  再次处理同一文件会显示「已处理过（历史记录跳过）：如需重新解压请点「重新解压」
  或在设置中关闭「跳过已处理」」并以成功计（exit 0），**不产生输出**。
  - 验证/重跑时不要开跳过；若之前开启过并写入了记录，可删除 `processed.json`、
    GUI 设置页「清除处理记录」，或改用另一个配置目录。
  - 旧 CLI 参数 `--no-skip-processed` 仍被接受，但已是兼容 no-op（默认本来就不跳过）。
  - GUI 对“全跳过”的批次显示提示条，提供「清除处理记录」/「全部重新解压」。
- `stop_on_error` 默认 false：单任务失败不终止整批；开启后首个失败即停止后续任务。
- 失败任务的输出不进入 processed 记录，可直接重试。
- 需要密码（`NeedsPassword`）不算失败输出：不写任何文件，CLI exit 1，
  GUI 标记为"需要密码"并可填口令后重试。

## 7. 内层/级联规则

- 只有"看起来是压缩包"的内层文件才会递归：由 7-Zip 列表或魔数判定，
  `max_depth` 默认 4（`RunConfig::effective_max_depth()` clamp 到 1..=8）。
  内层分卷同样支持（卷在 staging 内先聚合再展开）。
- SSEFE 解密出的明文 ZIP 视为一层归档（计 1 层），之后按普通压缩包处理。
  - 与 v0.1.x 的差异：旧 paranoia 只解密出内层 ZIP 就结束（用户自己再解压）；
    v0.2.0 会继续展开内层 ZIP（行为增强，`max_depth` 同样适用）。
- 深度耗尽后，最内层的归档文件**原样保留**为输出文件（不会报错丢失）。
- 只解 `.zip/.7z/.rar/.tar/.gz/.tgz/.bz2/.xz/.cab/.iso` 等 7-Zip 支持的格式；
  未知内层文件按普通文件落地。

## 8. 输出位置与命名边界

- 输入文件本身永远不会被修改/移动（只读打开）。
- `--dry-run`：完整走 detect/password 判定与列表，但不创建 staging、不落地、
  不写 processed 记录；退出码语义与真实运行一致（仍会因缺密码 exit 1）。
- 多输入时每个输入独立 plan；同一输出目录下多个同名顶层目录按 §3 合并/改名。
- 以 `.enc` 结尾的输入在命名时去掉 `.enc`；其他扩展名只去一个。

## 9. 相关代码索引

| 关注点 | 位置 |
|---|---|
| 目录计划 / 垃圾名 / 冲突搬运 | `crates/unpack-core/src/layout.rs` |
| 任务阶段 / staging / keep_broken / 空间检查 | `crates/unpack-core/src/engine.rs` |
| 队列、并发、暂停/取消、汇总 | `crates/unpack-core/src/queue.rs`、`cancel.rs` |
| 指纹、move/copy、临时文件 | `crates/unpack-core/src/util.rs` |
| 分卷家族识别/聚合/缺卷检测 | `crates/unpack-core/src/split.rs` |
| 7-Zip 调用、密码检测、列表解析 | `crates/unpack-core/src/sevenzip.rs` |
| SSEFE 解密与内层 ZIP 归一化 | `crates/unpack-core/src/ssefe.rs` |
| processed.json / settings.json（含 v0.3.2 `settingsVersion` 迁移） | `crates/unpack-core/src/processed.rs`、`settings.rs` |
| `related_paths`（分卷族归并，v0.3.2） | `crates/unpack-core/src/types.rs`、`engine.rs`（`TaskOutcome`） |
