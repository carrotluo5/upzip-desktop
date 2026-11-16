#!/usr/bin/env bash
# scripts/make-fixtures.sh — build the reproducible in-repo e2e fixtures.
# （可重复生成同样结构/内容，但**不是字节稳定**：7-Zip 记录 mtime、make_enc 用随机 salt。）
#
# Everything is created from files in this repository and the vendored 7-Zip
# binary at binaries/mac/7zz (macOS). No sample files, no network, no
# /Volumes paths. Output goes to testdata/fixtures/ (gitignored) and the
# directory is cleared on every run, so this script is re-runnable.
#
# Fixtures and what they exercise:
#   normal.zip        plain ZIP archive                       (simple extraction)
#   inner.zip         ZIP used as nesting payload
#   nested.zip        ZIP containing inner.zip                (nested-archive recursion)
#   encrypted.7z      7z with password 123 + encrypted headers (NeedsPassword flow)
#   encrypted-inner.7z 7z with password "inner"               (per-layer password retry)
#   masked.mp4        ZIP with a 4 KiB MP4-like prefix        (masked archive + prefix-ZIP carve)
#   sample.enc        Paranoia v4 container around hello.txt  (SSEFE decryption, pw 123)
#   nested.enc        Paranoia v4 container around nested.zip (decrypt -> nested extraction)
#   layered.enc       Paranoia v4 (pw "outer") around encrypted-inner.7z (pw "inner")
#                     (per-layer different passwords, candidate-pool retry)
#   split.7z.001..00N  split 7z volumes (1 MiB each; -mx=0 + 3 MiB blob)
#   split.7z.001.enc..  each split volume wrapped in its own Paranoia v4 container
#                     (pw 123; SSEFE split family -> decrypt all volumes, then extract)
#   truncated.7z.001.enc  first volume of a split family whose later volumes are absent
#                     (expected: explicit "missing later volumes" error, no partial output)
#
# Basic fixtures use password "123"; layered.enc intentionally uses two different
# passwords: outer SSEFE container "outer", inner 7z archive "inner".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SEVENZIP="$ROOT/binaries/mac/7zz"
FIX="$ROOT/testdata/fixtures"
WORK="$ROOT/testdata/.make-fixtures-work"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "make-fixtures: this script needs macOS (uses binaries/mac/7zz)" >&2
  exit 2
fi
if [[ ! -x "$SEVENZIP" ]]; then
  echo "make-fixtures: missing executable $SEVENZIP" >&2
  exit 1
fi

rm -rf "$FIX" "$WORK"
mkdir -p "$FIX" "$WORK/payload/docs"

printf 'hello unparanoia v0.3.1\n' > "$WORK/payload/hello.txt"
printf 'second file, used to check multi-file layouts\n' > "$WORK/payload/docs/readme.txt"

echo "== 1/10 normal.zip"
( cd "$WORK/payload" && "$SEVENZIP" a -tzip -mx=0 "$WORK/normal.zip" hello.txt docs/readme.txt >/dev/null )
cp "$WORK/normal.zip" "$FIX/normal.zip"

echo "== 2/10 inner.zip + nested.zip"
( cd "$WORK/payload" && "$SEVENZIP" a -tzip -mx=0 "$WORK/inner.zip" hello.txt >/dev/null )
( cd "$WORK" && cp "$WORK/inner.zip" "$WORK/nested-inner.zip" && "$SEVENZIP" a -tzip -mx=0 "$WORK/nested.zip" nested-inner.zip >/dev/null )
cp "$WORK/inner.zip" "$FIX/inner.zip"
cp "$WORK/nested.zip" "$FIX/nested.zip"

echo "== 3/10 encrypted.7z (password 123, encrypted headers)"
( cd "$WORK/payload" && "$SEVENZIP" a -t7z -p123 -mhe=on "$WORK/encrypted.7z" hello.txt docs/readme.txt >/dev/null )
cp "$WORK/encrypted.7z" "$FIX/encrypted.7z"

echo "== 4/10 encrypted-inner.7z (password inner, encrypted headers)"
( cd "$WORK/payload" && "$SEVENZIP" a -t7z -pinner -mhe=on "$WORK/encrypted-inner.7z" hello.txt >/dev/null )
cp "$WORK/encrypted-inner.7z" "$FIX/encrypted-inner.7z"

echo "== 5/10 masked.mp4 (4 KiB MP4-like prefix + ZIP payload)"
{ printf 'ftypisomisomiso2avc1mp41'; head -c $((4096 - 24)) /dev/zero | tr '\0' ' '; } > "$WORK/prefix.bin"
cat "$WORK/prefix.bin" "$WORK/normal.zip" > "$FIX/masked.mp4"

echo "== 6/10 sample.enc (make_enc hello.txt, password 123, customParams 00)"
cargo run -q --manifest-path "$ROOT/Cargo.toml" -p upzip-core --features testkit --example make_enc -- \
  "$WORK/payload/hello.txt" "$FIX/sample.enc" 123

echo "== 7/10 nested.enc (make_enc nested.zip, password 123, customParams 00)"
cargo run -q --manifest-path "$ROOT/Cargo.toml" -p upzip-core --features testkit --example make_enc -- \
  "$FIX/nested.zip" "$FIX/nested.enc" 123

echo "== 8/10 layered.enc (make_enc encrypted-inner.7z, password outer, customParams 00)"
cargo run -q --manifest-path "$ROOT/Cargo.toml" -p upzip-core --features testkit --example make_enc -- \
  "$FIX/encrypted-inner.7z" "$FIX/layered.enc" outer

echo "== 9/10 split.7z.001..004 (3 MiB stored payload, 1 MiB volumes)"
mkdir -p "$WORK/splitpayload"
cp "$WORK/payload/hello.txt" "$WORK/splitpayload/hello.txt"
head -c $((3 * 1024 * 1024)) /dev/zero > "$WORK/splitpayload/blob.bin"
( cd "$WORK/splitpayload" && "$SEVENZIP" a -t7z -mx=0 -v1m "$WORK/split.7z" hello.txt blob.bin >/dev/null )
cp "$WORK"/split.7z.* "$FIX"/
# 每个卷单独包一层 Paranoia v4（密码 123）: testdata/fixtures/split.7z.001.enc ...
for vol in "$WORK"/split.7z.*; do
  base="$(basename "$vol")"
  cargo run -q --manifest-path "$ROOT/Cargo.toml" -p upzip-core --features testkit --example make_enc -- \
    "$vol" "$FIX/${base}.enc" 123
done
# 只有首卷的分卷族（后续卷缺失）：期望引擎明确报错而不是产出半成品。
cp "$WORK/split.7z.001" "$WORK/truncated.7z.001"
cargo run -q --manifest-path "$ROOT/Cargo.toml" -p upzip-core --features testkit --example make_enc -- \
  "$WORK/truncated.7z.001" "$FIX/truncated.7z.001.enc" 123

rm -rf "$WORK"
echo
echo "== fixtures in $FIX"
( cd "$FIX" && shasum -a 256 * | tee "$FIX/SHA256SUMS" )
echo
echo "make-fixtures: OK"