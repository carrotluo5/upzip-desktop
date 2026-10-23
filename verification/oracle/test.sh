#!/usr/bin/env bash
# End-to-end independent verification of Paranoia File Encryption v4 AES-256 (.enc).
#
# Builds the Java oracle from source, decrypts BOTH sample files with password
# "123", checks the BLAKE3 keyed MAC / checkcode / ZIP CRC / plaintext SHA-256 /
# byte-identity with the originals, exercises the wrong-password path, and
# (re)writes test-vectors.json for the Rust tests.
#
# Env overrides:
#   JAVA_HOME     JDK 17+      (default: first /tmp/jdk21/*/Contents/Home)
#   PARANOIA_PC   vendored official source tree (default /tmp/paranoia-pc, READ-ONLY)
#   SAMPLES_DIR   dir containing the .enc samples + originals (default /Volumes/Carrot/Download)
#   OUTDIR        scratch output dir (default /tmp/paranoia-oracle-test)
#   JSON_OUT      test-vectors path (default: <this dir>/test-vectors.json)
#   CTR_ENGINE    bc (official BouncyCastle SICBlockCipher) | jce (JDK AES/ECB keystream)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

# --- locate JDK --------------------------------------------------------------
if [[ -z "${JAVA_HOME:-}" ]]; then
  for cand in /tmp/jdk21/*/Contents/Home /tmp/jdk21/*; do
    if [[ -x "$cand/bin/javac" ]]; then JAVA_HOME="$cand"; break; fi
  done
fi
if [[ -z "${JAVA_HOME:-}" || ! -x "$JAVA_HOME/bin/javac" ]]; then
  echo "ERROR: no JDK found; set JAVA_HOME (JDK 17+)" >&2
  exit 1
fi
JAVA="$JAVA_HOME/bin/java"

# --- inputs ------------------------------------------------------------------
SAMPLES_DIR="${SAMPLES_DIR:-/Volumes/Carrot/Download}"
OUTDIR="${OUTDIR:-/tmp/paranoia-oracle-test}"
JSON_OUT="${JSON_OUT:-$HERE/test-vectors.json}"
CTR_ENGINE="${CTR_ENGINE:-bc}"
PASSWORD="${PASSWORD:-123}"

S1="$SAMPLES_DIR/3138.7z.001.enc"
S2="$SAMPLES_DIR/3138.7z.002.enc"
O1="$SAMPLES_DIR/3138.7z.001"
O2="$SAMPLES_DIR/3138.7z.002"
EXPECT1="9d001d6c56ae12f29cf4244f6ac13c64f540b11b50df5ddecfebd0f9f058e40c"
EXPECT2="46abe32caa76660bc87ef9b52d4699d4129ff4fcd86994ccd80ac365d04ce840"

for f in "$S1" "$S2" "$O1" "$O2"; do
  [[ -f "$f" ]] || { echo "ERROR: missing input $f (set SAMPLES_DIR)" >&2; exit 1; }
done

mkdir -p "$OUTDIR"
rm -f "$OUTDIR/3138.7z.001.dec" "$OUTDIR/3138.7z.002.dec"

echo "== build =="
bash "$HERE/build.sh"

echo
echo "== inputs (read-only; samples never modified) =="
ls -l "$S1" "$S2"

echo
echo "== oracle: decrypt both samples + wrong-password negative test =="
"$JAVA" -cp "$HERE/build/classes" oracle.Oracle \
  --outdir "$OUTDIR" --json-out "$JSON_OUT" --ctr "$CTR_ENGINE" \
  --sample "3138.7z.002|$S2|$PASSWORD|$O2|$EXPECT2" \
  --sample "3138.7z.001|$S1|$PASSWORD|$O1|$EXPECT1" \
  --sample "3138.7z.002-wrongpassword-456|$S2|456|||wrongpw"

echo
echo "== independent post-checks (sha256 + cmp against originals) =="
if command -v sha256sum >/dev/null 2>&1; then SHACMD="sha256sum"; else SHACMD="shasum -a 256"; fi
fail=0
check_sha() {
  local label="$1" file="$2" expected="$3" actual
  actual="$($SHACMD "$file" | awk '{print $1}')"
  if [[ "$actual" == "$expected" ]]; then
    echo "PASS  $label sha256 = $actual"
  else
    echo "FAIL  $label sha256 = $actual (expected $expected)"; fail=1
  fi
}
check_cmp() {
  local label="$1" a="$2" b="$3"
  if cmp -s "$a" "$b"; then echo "PASS  $label byte-identical to original"; else echo "FAIL  $label differs from original"; fail=1; fi
}
check_sha "3138.7z.002" "$OUTDIR/3138.7z.002.dec" "$EXPECT2"
check_sha "3138.7z.001" "$OUTDIR/3138.7z.001.dec" "$EXPECT1"
check_cmp "3138.7z.002" "$OUTDIR/3138.7z.002.dec" "$O2"
check_cmp "3138.7z.001" "$OUTDIR/3138.7z.001.dec" "$O1"

if [[ $fail -ne 0 ]]; then echo; echo "VERIFICATION FAILED"; exit 1; fi
echo
echo "ALL CHECKS PASSED (MAC, checkcode, CRC, SHA-256, cmp, wrong-password)"
