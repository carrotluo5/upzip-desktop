# Paranoia File Encryption v4 — 文件格式说明（AES-256 / 密码模式）

> 本文档基于样本逆向 + 官方源码交叉确认，用于本项目实现与验收。

## 样本

| 文件 | 大小 (B) | SHA-256 (解密后明文) |
|---|---|---|
| 3138.7z.001.enc | 209779415 | 9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c（209715200 B）|
| 3138.7z.002.enc | 60527110 | 46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840（60508410 B）|

两文件均已被本项目实现逐字节还原（cmp 一致）。

## 整体布局

```
offset            内容
[0, 8)            明文头 8 字节: "SSEFE"      (5B magic, ASCII)
                              version        (1B, 样本=0x04)
                              algorithm      (1B, 0x00 = AES-256)
                              customParams   (1B, 样本=0x11, 编码 Argon2 参数)
[8, 40)           salt: 32B 随机盐
[40, L-32)        ciphertext: AES-256-CTR 加密的明文流
                    明文流 = checkcode(32B, 随机小写字母数字) || 内嵌 ZIP 归档
[L-32, L)         MAC: BLAKE3 keyed (32B), 覆盖 nonce||header||salt||ciphertext
```

L = 文件总长。样本 L=60527110 / 209779415。无任何额外字段；不含明文文件名。

## 口令 → 密钥 (V4)

1. 口令规整: 逐字符转换，可打印 ASCII (32..=126) 保留，其余字符替换为十进制码点数字串，再取 UTF-8 字节。
   （`"123"` => `b"123"`）
2. `l0PWHash = HKDF-Skein1024/1024(IKM=passwordBytes, salt="memorySalt", info="memoryInfo", L=256)`。
   HMAC 使用 Skein-1024 块大小 128B，输出 128B。
3. `argonOut = Argon2id(version=0x13, password=l0PWHash, salt=salt32, t=20, m=20480 KiB, p=4, out=256B)`。
   参数由 customParams 字节编码: `tm = low nibble`, `mm = high nibble`; `t = 10*2^tm`, `m = 10240*2^mm KiB`, `p = 4`（v4 固定）。
   官方 Java 实现（`Argon2Params.getParametersForFE`）对全部 256 个字节值都接受（极端值如 0xF1 会要求
   m = 320 GiB，实际运行会 OOM）。本实现出于安全考虑**主动拒绝** `tm > 6` 或 `mm > 6`（即 t > 640
   或 m > 640 MiB）的极端参数并报 `invalidCustomParams`，避免尝试天文数字的内存分配；这是本实现的
   安全护栏，不是格式规则。正常软件生成的样本为 `0x11`（t=20, m=20480 KiB）。
4. 三个子密钥（均为 HKDF-SHA3-512，IKM=argonOut）:
   - encKey32 = HKDF(salt="encKeySalt", info="encKeyInfo")
   - authKey32 = HKDF(salt="authKeySalt", info="authKeyInfo")
   - nonce16  = HKDF(salt="nonceSalt",  info="nonceInfo")

## 加密与完整性

- 算法: AES-256-CTR；初始计数器块 = 16B nonce（大端 128 位），每 16B 块 +1；无填充。
- 明文流首 32B 为 checkcode：由 `[a-z0-9]` 随机字符组成，用于快速判错口令。
  解密头 32 字节后若出现非 `[a-z0-9]` 字符 => 口令错误。
- MAC: BLAKE3 keyed 模式，key=authKey32，输入按顺序 = nonce(16) || header(8) || salt(32) || ciphertext 全部字节；对比文件末 32B。
  校验失败 => 文件损坏/被篡改（或口令错误）。
- 无嵌入恢复容器（PFTEREC1）的样本。若检测到该扩展（ciphertext 结束前 8B == "PFTEREC1"），一期明确报错不支持。

## 内嵌 ZIP（原始文件名来源）

- 明文流去掉 32B checkcode 后是一个标准 ZIP（Zip64 视需要）。
- 单文件加密时：ZIP 内仅含一个文件 entry，entry 名即原始文件名（如 `3138.7z.002`），UTF-8，Unix 分隔符；可含 Paranoia 私有 extra field（不重要）。
- 目录加密时：ZIP 内含目录树 entry，按相对路径恢复。
- 压缩方法: STORED(0) / DEFLATED(8) / ZSTD(93)。样本为 DEFLATED。
- 每个 entry 带 CRC32；解压时校验，失败 => 文件损坏。
- 解密产物文件名/扩展名从 entry 恢复；内容逐字节与原始文件一致。

## 错误判定顺序（实现约定）

1. 长度 < 72 => tooShort/truncated
2. magic != "SSEFE" => badMagic
3. version != 4 => unsupportedVersion
4. algorithm != 0 => unsupportedAlgorithm
5. customParams 超出本实现安全上限（tm>6 或 mm>6，见上文） => invalidCustomParams
   （格式本身允许该字节，官方实现会接受；这是防止 OOM 的实现护栏）
6. 恢复容器魔数 => unsupportedRecovery
7. checkcode 非法字符 => wrongPassword
8. MAC 不匹配 => integrity
9. ZIP CRC 失败 => zip
