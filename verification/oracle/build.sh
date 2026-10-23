#!/usr/bin/env bash
# Build the independent Java oracle.
#
# The cryptographic primitives are taken from the vendored official Paranoia
# source tree (read-only, never modified): javac compiles the needed classes
# from that source path on demand; all .class output goes to ./build/classes.
set -euo pipefail
HERE="$(cd $(dirname $0) && pwd)"

# JDK: JAVA_HOME, or the Temurin 21 installed during verification.
if [[ -z "${JAVA_HOME:-}" ]]; then
  for cand in /tmp/jdk21/*/Contents/Home /tmp/jdk21/*; do
    if [[ -x "$cand/bin/javac" ]]; then JAVA_HOME="$cand"; break; fi
  done
fi
if [[ -z "${JAVA_HOME:-}" || ! -x "$JAVA_HOME/bin/javac" ]]; then
  echo "ERROR: no JDK found; set JAVA_HOME (JDK 17+)" >&2
  exit 1
fi

VENDOR="${PARANOIA_PC:-/tmp/paranoia-pc}"
if [[ ! -d "$VENDOR/CryptoLibs/src" ]]; then
  echo "ERROR: vendored official source not found at $VENDOR (set PARANOIA_PC)" >&2
  exit 1
fi

OUT="$HERE/build/classes"
rm -rf "$OUT"
mkdir -p "$OUT"

echo "JDK     : $($JAVA_HOME/bin/javac -version 2>&1)"
echo "sources : $HERE/src"
echo "vendor  : $VENDOR/CryptoLibs/src (read-only)"
$JAVA_HOME/bin/javac -encoding UTF-8 -d "$OUT" \
  -sourcepath "$HERE/src:$VENDOR/CryptoLibs/src" \
  "$HERE/src/oracle/Oracle.java"
echo "built   : $OUT"
