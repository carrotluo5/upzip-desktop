# upzip-core — 内部 API 契约（v0.3.0）

本文件是 core crate 与 GUI/CLI 的接口约定。core 的公开 API 一旦写下即视为稳定；GUI 侧按此调用。

> 本文正文是 v0.3.0 契约的快照，其中 `CONFIG_DIR_NAME = "unparanoia"`、
> `LEGACY_CONFIG_DIR_NAME = "paranoia-enc-decryptor"`、`UNPARANOIA_*` 环境变量和
> `unparanoia-cli` 示例都是当时的写法。0.4.0 起的实际值是：crate `upzip-core`、
> `CONFIG_DIR_NAME = "upzip"`、`LEGACY_CONFIG_DIR_NAME = "unparanoia"`、
> 环境变量 `UPZIP_*`（旧名兼容读取）、CLI 二进制 `upzip-cli`。

## Crate

- 名称: `upzip-core` (path: `crates/core`)
- Rust edition 2021, 无 async、无网络、无 GUI 依赖，纯同步逻辑（GUI 在自己的线程里调用）。
- 所有错误类型实现 `std::error::Error + Display`，并暴露稳定的 `kind() -> &'static str` 供 GUI 本地化。

## 常量

```rust
pub const MAGIC: [u8; 5] = *b"SSEFE";
pub const FORMAT_VERSION: u8 = 4;
pub const ALGORITHM_AES_256: u8 = 0x00;
pub const HEADER_LEN: usize = 8;
pub const SALT_LEN: usize = 32;
pub const CHECKCODE_LEN: usize = 32;
pub const MAC_LEN: usize = 32;
```

## 数据结构

```rust
pub struct Header {
    pub version: u8,          // 4
    pub algorithm: u8,        // 0 = AES-256
    pub custom_params: u8,    // 0x11 等，KDF 参数编码
    pub salt: [u8; 32],
    pub raw: [u8; 8],         // 原始 8 字节头
}

pub struct Argon2Params {
    pub iterations: u32,      // t
    pub memory_kib: u32,      // m (KiB)
    pub parallelism: u32,     // p
    pub output_len: usize,    // 256
}

pub struct Keys {
    pub enc_key: [u8; 32],
    pub nonce: [u8; 16],
    pub auth_key: [u8; 32],
}

pub enum ConflictStrategy { Overwrite, Rename, Error }   // serde: "overwrite" | "rename" | "error"

pub struct DecryptOptions {
    pub output_dir: PathBuf,
    pub conflict: ConflictStrategy,
}

pub struct DecryptReport {
    pub input: PathBuf,
    pub outputs: Vec<PathBuf>,   // 实际写出的文件（单文件=1 个；目录归档可能多个）
    pub original_name: String,   // 从 zip entry 恢复的原始文件名
    pub bytes_written: u64,
}

pub struct FileOutcome {
    pub input: PathBuf,
    pub outputs: Vec<PathBuf>,
    pub original_name: Option<String>,
    pub error_kind: Option<String>,     // Some => 失败
    pub error_message: Option<String>,
}
impl FileOutcome { pub fn is_ok(&self) -> bool; }

pub struct BatchOptions {
    pub files: Vec<PathBuf>,
    pub password: String,
    pub output_dir: Option<PathBuf>,     // None => 默认: 第一个文件的父目录
    pub conflict: ConflictStrategy,
}

pub struct BatchSummary {
    pub outcomes: Vec<FileOutcome>,
    pub successes: usize,
    pub failures: usize,
    pub output_dir: PathBuf,
}

pub struct BatchProgress {
    pub index: usize,             // 0-based 当前文件
    pub total: usize,
    pub current_file: PathBuf,
    pub file_processed: u64,
    pub file_total: u64,
    pub overall_processed: u64,
    pub overall_total: u64,
    pub successes: usize,
    pub failures: usize,
}
```

## 函数

```rust
// ---- 校验/解析（添加文件时用，不读全文件） ----
pub fn parse_header(bytes: &[u8]) -> Result<Header, DecryptError>;
pub fn parse_argon2_params(custom_params: u8) -> Result<Argon2Params, DecryptError>;
pub fn inspect_file(path: &Path) -> Result<Header, DecryptError>;  // 读前 40 字节
pub fn validate_input_path(path: &Path) -> Result<Header, AddFileError>; // 扩展名 + 头 + 40B 长度

// ---- KDF（可单独测试） ----
pub fn derive_l0_password_hash(password: &str) -> Vec<u8>;                 // 256B, 供测试
pub fn derive_keys(password: &str, salt: &[u8; 32], params: &Argon2Params) -> Result<Keys, DecryptError>;

// ---- 单文件解密 ----
// progress(processed_bytes_out_of_ciphertext): 每读一个 chunk 回调一次（含末尾 total,total）
pub fn decrypt_file(
    input: &Path,
    password: &str,
    opts: &DecryptOptions,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<DecryptReport, DecryptError>;

// ---- 批量（失败隔离；输出目录解析；GUI 用） ----
pub fn default_output_dir(files: &[PathBuf]) -> Option<PathBuf>;
pub fn resolve_output_dir(files: &[PathBuf], custom: Option<&Path>) -> Option<PathBuf>;
pub fn run_batch(opts: &BatchOptions, progress: &mut dyn FnMut(BatchProgress)) -> BatchSummary;

// ---- 密码库（多密码明文；OS 用户配置目录，Unix 0600） ----
pub mod config {
    pub const CONFIG_FILE_NAME: &str = "config.json";
    pub const CONFIG_DIR_NAME: &str = "unparanoia";
    pub const LEGACY_CONFIG_DIR_NAME: &str = "paranoia-enc-decryptor";

    pub fn config_dir() -> Result<PathBuf, ConfigError>;        // UNPARANOIA_CONFIG_DIR > PARANOIA_ENC_CONFIG_DIR（旧名回退）> 默认目录
    pub fn config_file_path() -> Result<PathBuf, ConfigError>;
    pub fn legacy_config_dir() -> Option<PathBuf>;              // 旧 v0.2.0 目录，仅用于迁移读取

    pub fn load_passwords() -> Result<Vec<String>, ConfigError>; // 去空、去重、保序；缺文件返回空
    pub fn save_passwords(passwords: &[String]) -> Result<(), ConfigError>;
    pub fn add_password(password: &str) -> Result<bool, ConfigError>;   // 空串或已存在 => false
    pub fn remove_password_at(index: usize) -> Result<bool, ConfigError>; // 0 基；越界 => false
    pub fn clear_passwords() -> Result<(), ConfigError>;         // 删除 config.json（幂等）

    // v0.2.0 兼容 API（源码兼容；底层即上面的密码库）
    pub fn load_remembered_password() -> Result<Option<String>, ConfigError>; // 返回第一个
    pub fn save_remembered_password(password: &str) -> Result<(), ConfigError>; // 覆盖为单元素
    pub fn clear_remembered_password() -> Result<(), ConfigError>;
}
```

密码库文件格式（明文）：

```json
{ "passwords": ["123", "family-share"] }
```

- 写入是「临时文件 + rename」，Unix 下权限 0600；Windows 没有对应权限位（已知限制）。
- **迁移**：首次读取时若新 `config.json` 不存在、而旧目录
  `paranoia-enc-decryptor/config.json` 存在，则读取其中的 `passwords` 与
  `remembered_password`/`rememberedPassword`（v0.2.0 单值），去重后写入新文件；
  旧文件不删除、不修改。`legacy_config_dir()` 同时接受旧环境变量
  `PARANOIA_ENC_CONFIG_DIR` 指向的目录（便于旧脚本/便携包迁移）。
- GUI 命令 `list_passwords/add_password/remove_password/clear_passwords` 直接映射上述函数；
  CLI `config --list-passwords/--add-password/--remove-password/--clear-passwords` 同理。

## DecryptError（kind 字符串）

```rust
pub enum DecryptError {
    Io(std::io::Error),               // kind: "io"
    TooShort,                         // "tooShort"       文件过短/被截断（完整解密 < 104 字节；inspect 仅需 >= 40）
    Truncated,                        // "truncated"      密文长度与头部声明不符 / 被截断
    BadMagic,                         // "badMagic"       不是 SSEFE 文件
    UnsupportedVersion(u8),           // "unsupportedVersion"
    UnsupportedAlgorithm(u8),         // "unsupportedAlgorithm"
    InvalidCustomParams(u8),          // "invalidCustomParams"
    UnsupportedRecovery,              // "unsupportedRecovery"  检测到 PFTEREC1 恢复容器（一期不支持）
    WrongPassword,                    // "wrongPassword"  口令校验码不匹配
    Integrity,                        // "integrity"      BLAKE3 MAC 不匹配（损坏/篡改）
    Zip(String),                      // "zip"            zip 解析/CRC 失败
    OutputExists(PathBuf),            // "outputExists"   冲突策略=Error 且目标已存在
    InvalidOutputName(String),        // "invalidOutputName"  zip 内路径非法（zip-slip 等）
    Crypto(String),                   // "crypto"         内部加密错误
}
pub enum AddFileError { NotEncExtension, CannotRead(std::io::Error), TooShort, BadMagic, UnsupportedVersion(u8), UnsupportedAlgorithm(u8), InvalidCustomParams(u8) }
```

## CLI（crates/cli，完整用法见 README / `--help`）

v0.3.0 起 CLI 是产品的一部分（不再只是验证工具），bin 名为 `unparanoia-cli`，子命令：

```text
unparanoia-cli [FILES...]            等价于 run（兼容旧用法）
unparanoia-cli run [PATH...]         批量解密/解压
unparanoia-cli detect PATH...        只识别（可 --json，输出 Detection）
unparanoia-cli inspect FILE...       打印 SSEFE 头信息
unparanoia-cli config [--show|--path|--list-passwords|--add-password PW|
                       --remove-password IDX|--clear-passwords|
                       --set-password PW|--clear-password]
unparanoia-cli doctor                7-Zip/配置目录/自检
unparanoia-cli cleanup-staging [--root DIR]...
```

`config` 说明：`--list-passwords` 逐条打印密码库（默认掩码，具体格式以 `--help` 为准）；
`--add-password` 追加、`--remove-password INDEX` 删除 0 基索引、`--clear-passwords` 清空；
`--set-password`/`--clear-password` 是 v0.2.0 单密码用法，保留兼容（单密码即覆盖为一项/清空）。

兼容用法（不写子命令的位置参数形式）在 v0.3.0 仍可用；与 v0.1.1 的差异是
它也会读取 config.json 的密码库（旧版只认 `-p`/环境变量）。

`run` 主要选项：`-p/--password`（可重复）、`-o/--output-dir`、`-j/--jobs`、
`--conflict rename|skip|overwrite`、`--layout smart|folder|direct`、`--no-recursive`、
`--no-nested`、`--max-depth 1-8`、`--keep-broken`、`--dry-run`、`--force-documents`、
`--allow-execute-sfx`、`--stop-on-error`、`--skip-processed`（默认不跳过；旧
`--no-skip-processed` 仍接受但为兼容 no-op）、`--no-strip-junk`、
`--try-empty-password`、`--json`、`--sevenzip PATH`。

退出码: 0=全部成功/跳过；1=存在失败或需要密码；2=用法/配置/环境错误；130=被中断。
`--json` 输出纯 JSON Lines（若干 TaskEvent + 末行 `{"type":"summary",...}`）。
