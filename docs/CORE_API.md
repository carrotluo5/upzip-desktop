# paranoia-enc-core — 内部 API 契约（Lead 维护）

本文件是 core crate 与 GUI/CLI 的接口约定。core 的公开 API 一旦写下即视为稳定；GUI 侧按此调用。

## Crate

- 名称: `paranoia-enc-core` (path: `crates/core`)
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

// ---- 密码记忆（明文；OS 用户配置目录） ----
pub mod config {
    pub fn config_file_path() -> Option<PathBuf>;
    pub fn load_remembered_password() -> Option<String>;
    pub fn save_remembered_password(password: &str) -> Result<(), ConfigError>;
    pub fn clear_remembered_password() -> Result<(), ConfigError>;
}
```

## DecryptError（kind 字符串）

```rust
pub enum DecryptError {
    Io(std::io::Error),               // kind: "io"
    TooShort,                         // "tooShort"       文件过短/被截断（< 72 字节）
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

## CLI（crates/cli，仅供验证/无头用途）

```
paranoia-enc-cli [OPTIONS] <FILES>...
  -p, --password <PASSWORD>       必填（除非 --inspect/--config）
  -o, --output-dir <DIR>          缺省 = 首个文件所在目录
      --conflict <overwrite|rename|error>   默认 rename
      --json                      输出机器可读结果（JSON 一行）
      --inspect                   只打印每文件头部信息
      --config-path               打印密码存储路径
      --clear-password            清除记忆密码
      命令形式: config show | config clear
```

退出码: 0=全部成功；1=存在失败文件；2=参数/IO 错误。
