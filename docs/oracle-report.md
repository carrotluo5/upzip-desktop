# Oracle Verification Report — Paranoia File Encryption v4 (AES-256 / .enc)

**Date:** 2026-10-23
**Author:** independent verification engineer (teammate `oracle`)
**Scope:** build a non-Rust, independent implementation of the v4 container format, cross-check the format understanding documented in `docs/FORMAT.md`, and validate decryption of both sample files against the expected plaintext SHA-256 values and the original files.
**Artifacts:** `verification/oracle/` (Java oracle, `build.sh`, `test.sh`, `test-vectors.json`, `README.md`), this report. Samples and the vendored official source tree were treated as read-only.

---

## 1. Verdict

**Both samples fully verified. No cryptographic or layout discrepancies found.**

| Sample | Enc size (B) | BLAKE3 keyed MAC | Checkcode | Inner ZIP CRC | Decrypted SHA-256 | `cmp` vs original |
|---|---|---|---|---|---|---|
| 3138.7z.002.enc | 60,527,110 | **match** | valid (32 chars `[a-z0-9]`) | match | `46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840` | **identical** |
| 3138.7z.001.enc | 209,779,415 | **match** | valid | match | `9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c` | **identical** |

Wrong password **"456"** on sample 002 → decrypted checkcode contains non-`[a-z0-9]` bytes → rejected as **wrong password** (BLAKE3 MAC also fails, as expected). This confirms the checkcode-first error semantics in FORMAT.md.

The documented format understanding (header/salt/ciphertext/MAC layout, HKDF-Skein-1024 → Argon2id → HKDF-SHA3-512 key schedule, AES-256-CTR counter semantics, BLAKE3 keyed MAC input ordering, embedded ZIP + checkcode plaintext stream) is **confirmed independently**.

---

## 2. Environment

| Item | Value |
|---|---|
| Machine | macOS 27.2 (Build 26B118), arm64 |
| JDK | **Eclipse Temurin 21.0.13+9** (`openjdk version "21.0.13" 2026-10-20 LTS`), installed from the Adoptium 21 GA macOS/aarch64 tarball to `/tmp/jdk21/jdk-21.0.13+9/Contents/Home` (no system JDK was present; `/usr/bin/java` was a stub). |
| Vendored official source | `/tmp/paranoia-pc` (Paranoia File Encryption / SSE source tree), **read-only, unmodified**; used via `javac -sourcepath`, all class output written to `verification/oracle/build/classes`. |
| Samples | `/Volumes/Carrot/Download/3138.7z.001.enc`, `…002.enc` (read-only; never modified). Originals `3138.7z.001` / `.002` used only for `cmp`. |
| Password | `"123"` (ASCII) |

Recorded sample input hashes (unchanged by verification; files only opened for reading):

```
sha256 3138.7z.001.enc = 078386d3ad550d438dcaa2f6297e5b75112e4fbaea6731685d56bc7e8254877c  (209779415 B)
sha256 3138.7z.002.enc = 42ef9720fef2b13cea7548be26261ce0b29df07aaddeb06208a70fbf7c9c9623  ( 60527110 B)
```

---

## 3. Official code vs. independent reimplementation

This is the load-bearing part of the cross-check — what is genuinely independent.

**Reused (read-only) from the vendored official tree**, compiled on demand by
`javac -sourcepath` (sources never edited):

| Component | Path under `/tmp/paranoia-pc` |
|---|---|
| Skein-1024 digest, HKDF, SHA3-512 | `CryptoLibs/src/sse/org/bouncycastle/crypto/…` (`SkeinDigest`, `HKDFBytesGenerator`, `SHA3Digest`) |
| AES engine, CTR (SIC) mode | `CryptoLibs/src/sse/org/bouncycastle/crypto/engines/AESEngine.java`, `…/modes/SICBlockCipher.java` |
| BLAKE3 keyed hash | `CryptoLibs/src/sse/org/bouncycastle/crypto/digests/Blake3Digest.java`, `…/params/Blake3Parameters.java` |
| Argon2id | `CryptoLibs/src/at/gadermaier/argon2/…` (the exact implementation shipped by the app) |

**Independently reimplemented** in `verification/oracle/src/oracle/Oracle.java`
(written from `docs/FORMAT.md` plus the official Java sources; the Rust
implementation was **not** consulted, imported, or executed):

- container header parsing/validation (magic `SSEFE`, version 4, algorithm 0,
  customParams byte → Argon2 t/m/p);
- KDF wiring: password normalization → HKDF-Skein1024/1024
  (salt `memorySalt`, info `memoryInfo`, 256-byte output) → Argon2id v1.3
  t=20 m=20480 KiB p=4 out=256 B → HKDF-SHA3-512 to encKey32 / authKey32 / nonce16
  with the documented salts/infos;
- AES-256-CTR streaming where the 16-byte nonce is the initial counter block,
  big-endian, +1 per 16-byte block, no padding;
- BLAKE3 keyed MAC over `nonce(16) || header(8) || salt(32) || ciphertext`
  compared with the trailing 32 bytes;
- checkcode rule (`[a-z0-9]`, 32 bytes, wrong-password detection);
- embedded ZIP extraction with CRC32 verification, plaintext SHA-256, byte
  comparison against the original file, JSON test-vector emission.

So the crypto primitives are the official ones (no risk of a subtly different
Skein/BLAKE3/Argon2), while all container-level understanding is a second,
independent implementation. That is exactly the axis the Rust implementation
could get wrong.

Two independent CTR implementations were used to rule out counter-mode errors:

- `--ctr bc` (default): official `SICBlockCipher`;
- `--ctr jce`: JDK `AES/ECB/NoPadding` keystream with the same big-endian
  counter increment logic.

---

## 4. Commands executed

```bash
# 1) JDK (Temurin 21 GA, macOS arm64) from Adoptium, unpacked to /tmp/jdk21
#    (no system JDK; install is external to the repo)
JAVA_HOME=/tmp/jdk21/jdk-21.0.13+9/Contents/Home
"$JAVA_HOME/bin/javac" -version   # javac 21.0.13

# 2) build the oracle (reuses official sources read-only via -sourcepath)
bash verification/oracle/build.sh

# 3) full end-to-end verification (both samples + wrong-password negative test,
#    then independent sha256 + cmp post-checks; rewrites test-vectors.json)
bash verification/oracle/test.sh

# 4) manual form (equivalent; what test.sh runs)
"$JAVA_HOME/bin/java" -cp verification/oracle/build/classes oracle.Oracle \
  --outdir /tmp/oracle-out --json-out verification/oracle/test-vectors.json --ctr bc \
  --sample "3138.7z.002|/Volumes/Carrot/Download/3138.7z.002.enc|123|/Volumes/Carrot/Download/3138.7z.002|46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840" \
  --sample "3138.7z.001|/Volumes/Carrot/Download/3138.7z.001.enc|123|/Volumes/Carrot/Download/3138.7z.001|9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c" \
  --sample "3138.7z.002-wrongpassword-456|/Volumes/Carrot/Download/3138.7z.002.enc|456|||wrongpw"

# 5) CTR engine cross-check on sample 002 (JDK AES-CTR vs official SIC)
"$JAVA_HOME/bin/java" -cp verification/oracle/build/classes oracle.Oracle \
  --outdir /tmp/oracle-out-jce --ctr jce \
  --sample "jce-ctr-002|/Volumes/Carrot/Download/3138.7z.002.enc|123||46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840"
cmp /tmp/oracle-out/3138.7z.002.dec /tmp/oracle-out-jce/jce-ctr-002.dec   # identical

# 6) customParams parsing evidence
"$JAVA_HOME/bin/java" -cp verification/oracle/build/classes oracle.Oracle --parse-custom-params
```

---

## 5. Results

### 5.1 Header (both samples)

```
headerHex = 5353454645040011
            "SSEFE"  version=0x04  algorithm=0x00 (AES-256)  customParams=0x11
argon2    = Argon2id v0x13 t=20 (10*2^1)  m=20480 KiB (10240*2^1)  p=4  out=256 B
```

### 5.2 Sample 3138.7z.002.enc — 60,527,110 B

```
salt            f0131e597c16317aeb749495f04593df2b0ac15a4e2f0768c7a493f7abe7e210
l0PWHashV3      27047a1d24373bdfb9c66d7222f284368a2a519e9567ff3dd13e36ffce31ecd0
                2591c5d6bb5aaac1db9719150f207a397d877764f457797751b2aa15223382677e
                50936f34e3a7475eb6c02054a3dd0938811e7b170c7cedb8dc7c13d471e2805e20
                c9c0994238a0b6d880faa868112070bc7ffc06cdb3d4f80375feae1a663b44b99
                232e2fbc571a96e02be65f481da55b96f894649feb509a110736837aa966a3ab157
                65bc2ef35e1960dbd9e40da50e91577c6dd374c17440618e30c5a8c800f6b8de1
                1dedc98e668b37cf3bcd7cc4d528a9476c0b0038309c697b4db4b0858bc6cb6c1
                24402d19ec48585cb10a657e2a1cc3fa24b316bdcea3df04ee5de0   (128 B)
argon2Output    92cd962f260e7f057b5b8cbc53d6ed99662562e5bc07f110974f97795dfcf798
                24b84e38df538a39177d1ad055b9c8d8ae7a37cbe9623ae73e99d0519527628e62
                c8544dba67aaeacf4a0b8342559f7207455e4d14f849330408e582cb818686a1ed
                6d82963941847f357d5c5dcb124009b5f83b8aa7b5363de5e1aa76a87b1766740c
                ea6ea0248ebd6eceaf899b97c6a6eb8c71c7aa3b78702bf3a7ff2a28125e55cf11
                448918b4f506e4ad013956a09295c5e95bccbb6c17f9d360b39b8fa38107c79a9b
                dba2d3ff4bb334d9993ea5ce0db79adea33c216a4f9f969ab6e382a20141d5b34
                9909e302b294503ce48acbf0e2ce9dd0fc8dbe1e58b22428f7638   (256 B)
encKey          30aec64bf99f6e090819de3bf62f930047594061842b1a4840d7d640a6940040
nonce           6bee60b5c5db84b3ec5fe7d871df5a72
authKey         047e74664e13be5ceb02f78f37553d7cc7753941003a7299ec526a124cc1748c
stored MAC      879150e2db78157febbfd69278708ed536da39679453eca55f97805b4c6bd0ae
computed MAC    879150e2db78157febbfd69278708ed536da39679453eca55f97805b4c6bd0ae  (match)
checkcode hex   376a716331717579307764673734737a74346265376c6d68647164346e69686c
checkcode       "7jqc1quy0wdg74szt4be7lmhdqd4nihl"  (32 chars, [a-z0-9])
zip entry       name="3138.7z.002"  uncompressed=60508410  crc32=69b560a4  crcMatch=true
plaintextLen    60527038 = 32 (checkcode) + 60527006 (inner zip)
decrypted sha   46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840  (MATCH)
cmp vs original IDENTICAL
timings         total 1.49 s, KDF 0.43 s  (unloaded machine; later runs under load ~7–22 s)
```

### 5.3 Sample 3138.7z.001.enc — 209,779,415 B

```
salt            7dbe8c5f3f6144c9993687d7e68820597b68c4b04379eb40520841a94f144ff0
l0PWHashV3      (identical to sample 002 — password-only HKDF, before salt is used)
argon2Output    3a387ab13cd90005420340e38ed52fbe968e2cd37589cb11c8bdf50eaa0d385b
                c5f1e35a08693b4025709ebc0859afaa61f10ef13391ef3d5e3e9425b78416d736
                fdc4ce70e0c58db29195ad559a94a33064349cfde919dbb24209b08e41ccb7a118
                7bad696da8b52537def06991b47ba40d5337684285c74b3374ca5d3eee75b0cd
                636d9a12a29c9d2d12c66abe557d287f833e03a084aae06d0a30f575c8f652f8
                2561c83127e1c3fd0e7a4f389863d9cb6ac3d8048df2339d36d1fd8e464c700
                defee917ac9702f1e90fd2ab50f47313e4b2608754b057c1a0a1d9dd70be22fa
                f43089ddd86699a62e928299335d4cb8b36519a09d635ebe9f752e29df423   (256 B)
encKey          e35cac8a57d2032cda807ac50ece8f98e534066916ad5dabd606529347551067
nonce           cbfe3765d84ff216c3d3295f6c71bd36
authKey         b923504494793946dd14c94e4d42292cda7073fd927ef648dda32c34564c59e8
stored MAC      4125d2791c69d981ee5a8bf7be9b42be30286a7887037d74a89d879d3054056c
computed MAC    4125d2791c69d981ee5a8bf7be9b42be30286a7887037d74a89d879d3054056c  (match)
checkcode hex   726c6c797a3675357632676330737a72686c3967726e66776837326b32626837
checkcode       "rllyz6u5v2gc0szrhl9grnfwh72k2bh7"
zip entry       name="3138.7z.001"  uncompressed=209715200  crc32=e4d028e5  crcMatch=true
decrypted sha   9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c  (MATCH)
cmp vs original IDENTICAL
timings         total 3.03 s, KDF 0.16 s  (unloaded)
```

(The identical `l0PWHashV3` for both samples confirms the documented order:
password-only HKDF-Skein happens before the per-file random salt enters Argon2id.)

### 5.4 Wrong-password test (sample 002, password `"456"`)

```
encKey (wrong pw)  646cc73f238cf7b1a32732fa26524c39d10a88de2ed1fef1aed578a7fcc5fe62
nonce (wrong pw)   6a3fad6429af95b2c5a0b59003be9277
authKey (wrong pw) 7f3df0ed411c8e1143682b9e4994e86e1beadd33b8a7146e3863d2cac3c7c8e1
stored MAC         879150e2db78157febbfd69278708ed536da39679453eca55f97805b4c6bd0ae
computed MAC       8acb3a74a7ca57d447ac1a5843c46c39e8adf21aa7676ffb1a58521f8e1358ee  (mismatch)
checkcode raw hex  ce7410e3bcc8a96a70fcdd95d70560347cc467034aae31b53b2bde8bb3ecbd35
                   -> not [a-z0-9]: rejected as wrong password (as FORMAT.md order #7)
```

### 5.5 CTR engine cross-check (sample 002)

```
--ctr bc  (official SICBlockCipher)   -> /tmp/oracle-out/3138.7z.002.dec          sha256 46ab…e840
--ctr jce (JDK AES/ECB keystream)    -> /tmp/oracle-out-jce/jce-ctr-002.dec     sha256 46ab…e840
cmp of the two decrypted files: byte-identical
```

This validates the counter-block construction (nonce as initial counter, big-endian
+1, no padding) independently of the official SIC implementation.

### 5.6 customParams byte parsing (bonus)

The official v4 parser `Argon2Params.getParametersForFE` (see
`/tmp/paranoia-pc/SSEBaseEngine/src/com/paranoiaworks/sse/Argon2Params.java:112-115`
`getMultipliersFromParamsByte`, call sites `Encryptor.java:1991-1996`) maps
**every** byte value as:

```
tm = byte & 0x0F          t = 10 * 2^tm
mm = (byte >> 4) & 0x0F   m = 10240 * 2^mm  KiB
p  = 4 (fixed for v4)     version = 0x13, out = 256 B
```

There is **no range/validity check**: all 256 values parse structurally.
`--parse-custom-params` prints the full table. Notable extremes:

| byte | t | m | note |
|---|---|---|---|
| `0x11` (samples) | 20 | 20,480 KiB | matches samples |
| `0x00` | 10 | 10,240 KiB | fastest accepted |
| `0x1F` | 327,680 | 20,480 KiB | absurdly slow |
| `0xF1` | 20 | 335,544,320 KiB (320 GiB) | Argon2 allocation failure / OOM likely |
| `0xFF` | 327,680 | 320 GiB | both extremes |

Consequence for FORMAT.md's error ordering: see §6.1.

### 5.7 Test vectors

`verification/oracle/test-vectors.json` (regenerated by `test.sh`) contains, for
both samples: header/salt hex, `argon2Params`, the full KDF chain hexes
(`l0PWHashV3Hex`, `argon2OutputHex`, `encKeyHex`, `nonceHex`, `authKeyHex`),
stored/computed MAC hex, checkcode hex + string, ZIP entry metadata + CRC,
decrypted SHA-256/size, `cmp` result — plus `negativeTests` for the wrong-password
run. This is the fixture set the Rust tests can assert against directly.

---

## 6. Discrepancies and caveats

**No discrepancies were found in the crypto or the container layout.** Both
samples decrypt correctly and match their originals byte-for-byte. Caveats,
stated honestly:

### 6.1 `invalidCustomParams` is not a structural parse error in official v4 code

FORMAT.md's error ordering lists a customParams parse failure (`invalidCustomParams`).
The official v4 Java code performs **no structural check**: every byte
`0x00..0xFF` yields a (possibly insane) t/m pair (evidence in §5.6); extreme
bytes fail only later, by resource exhaustion inside Argon2id.

**Resolution (clarified with the Lead, now documented in `docs/FORMAT.md`):** the
Rust implementation deliberately rejects `tm > 6` or `mm > 6` (t > 640 passes,
m > 640 MiB) and reports `invalidCustomParams`. This is an **implementation
safety guard** (it prevents pathological allocations / OOM when a user opens a
hostile file), **not** a Paranoia format rule — a strictly format-conformant
reader accepts all 256 byte values. FORMAT.md now states this explicitly in its
customParams section and error-ordering item #5. This does **not** affect either
sample (both use `0x11` → t=20, m=20480 KiB).

### 6.2 Error ordering only partially exercised end-to-end

The oracle implements the FORMAT.md ordering (length < 72, magic, version,
algorithm, customParams, recovery magic, checkcode, MAC, ZIP CRC), but for real
samples only order #7 (checkcode/wrong password) is end-to-end exercised. The
other branches are implemented but were not exercised against synthetic corrupt
files, and I did not separately verify the official code's ordering beyond the
relevant read paths. This is a coverage gap, not an observed mismatch.

### 6.3 v4 only

The samples are format version 4. The official code also supports older format
versions (e.g. different MAC key length 64 vs 32, see `Encryptor.java:1994-1996`).
The oracle intentionally implements v4 only; no v3 sample was available, so
nothing here validates a v3 claim.

### 6.4 Recovery extension not implemented

FORMAT.md mentions PFTEREC1 recovery-container detection ("一期明确报错不支持").
The samples contain no recovery trailer, and the oracle does not implement that
detection. Not exercised, not a problem for these samples.

### 6.5 Non-ASCII password normalization untested

The oracle implements the documented normalization (printable ASCII 32–126 kept;
other characters replaced by decimal codepoint digit strings before UTF-8), but
the samples use ASCII `"123"` only, so only that path is verified.

### 6.6 Informational: official file-manager detection is not validation

`CryptFile.java` (`ENC_FILE_HEADER_PREFIX = "SSE"`, `processCommon()`) only
checks that the first 3 bytes are `SSE` case-insensitively (plus an embedded-
recovery fallback) when deciding "is this an encrypted file" for UI purposes.
That is not the format validator, so the strong layout checks belong to the
decrypt path / FORMAT.md. No conflict with FORMAT.md, just context.

---

## 7. Independent conclusions

1. **FORMAT.md v4 layout is correct** for the provided samples: 8-byte header,
   32-byte salt, ciphertext from offset 40 to `L-32`, trailing 32-byte BLAKE3
   keyed MAC over `nonce || header || salt || ciphertext`; the plaintext stream is
   `checkcode(32B [a-z0-9]) || standard ZIP` with the original filename as the
   single ZIP entry name.
2. **KDF schedule is correct**: HKDF-Skein1024/1024 (`memorySalt`/`memoryInfo`,
   256 B) → Argon2id v1.3, t=20, m=20480 KiB, p=4, out=256 B (customParams
   `0x11`) → HKDF-SHA3-512 to `encKey32` (`encKeySalt`/`encKeyInfo`),
   `authKey32` (`authKeySalt`/`authKeyInfo`), `nonce16`
   (`nonceSalt`/`nonceInfo`). The `l0PWHashV3` value is identical for both
   samples (password-only), and the per-sample salt enters at Argon2id.
3. **AES-256-CTR semantics are correct**: nonce = initial counter block,
   big-endian, +1 per block, no padding — confirmed by two independent CTR
   implementations producing byte-identical plaintext.
4. **The Rust implementation's claimed results are independently corroborated**:
   the Java oracle, sharing only the vendored crypto primitives but none of the
   Rust container code, reproduces exactly the expected SHA-256 values
   (`46ab…e840` / `9d00…e40c`) and byte-identical outputs. There is no plausible
   "both implementations made the same mistake" explanation left for these
   fields, since the MAC key material and checkcode path are cross-checked
   independently.
5. **Rust tests can use `verification/oracle/test-vectors.json`** to assert the
   full KDF chain per sample (not only final plaintext), which pinpoints which
   stage diverges if the Rust implementation changes later.

**Recommendation to the Lead:** add Rust unit tests asserting the `encKey`,
`nonce`, `authKey`, and `computedMacHex` values from `test-vectors.json` for
both samples, plus the decrypted SHA-256. If any Rust intermediate hex differs
from the vectors, the divergence is in that stage of the KDF, not in the final
plaintext comparison.

---

## 8. Reproduce everything

```bash
export JAVA_HOME=/tmp/jdk21/jdk-21.0.13+9/Contents/Home   # or any JDK 17+
export PARANOIA_PC=/tmp/paranoia-pc                      # vendored official source, read-only
bash verification/oracle/build.sh
bash verification/oracle/test.sh
# Expected tail: "ALL CHECKS PASSED (MAC, checkcode, CRC, SHA-256, cmp, wrong-password)"
```

See `verification/oracle/README.md` for flags (`--ctr bc|jce`, `--json-out`,
`--parse-custom-params`, `--sample` spec) and the test-vector schema.
