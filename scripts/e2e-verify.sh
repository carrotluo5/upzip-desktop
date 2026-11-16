#!/usr/bin/env bash
#
# unparanoia — CLI 端到端验证（e2e）
#
# 在仓库内生成的 fixture 上跑真实 CLI（调用随附 7-Zip），断言:
#   退出码 / 输出布局 / 文件内容 sha256 / JSON Lines / 密码流程 /
#   nested 级联 / conflict 策略 / dry-run / cleanup-staging / config /
#   分段压缩包（普通分卷、SSEFE .enc 分卷、只给非首卷、缺后续卷）/
#   跳过已处理（v0.3.2 默认关闭、--skip-processed 才启用、related_paths）。
#
# 用法: ./scripts/e2e-verify.sh
#   - 需要 testdata/fixtures/（缺失时自动调用 scripts/make-fixtures.sh，需要 macOS + binaries/mac/7zz）
#   - 使用独立的临时配置目录（不碰用户真实 config.json / processed.json）
#   - 成功时最后一行输出: ALL E2E CHECKS PASSED
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

FIX="${ROOT}/testdata/fixtures"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/unparanoia-e2e.XXXXXX")"
export UPZIP_CONFIG_DIR="${TMP}/config"
mkdir -p "${UPZIP_CONFIG_DIR}"
# 隔离历史配置导入：首次加载时会把旧版 unparanoia 配置目录里的密码库导入进来，
# 若本机存在旧配置，错误口令/密码库用例会被污染（CI 干净机器上无此问题，本地却会）。
export PARANOIA_ENC_CONFIG_DIR="${TMP}/legacy-unparanoia"

cleanup() { rm -rf "${TMP}"; }
trap cleanup EXIT

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); printf '  \033[1;32mPASS\033[0m %s\n' "$*"; }
fail() { FAIL=$((FAIL + 1)); printf '  \033[1;31mFAIL\033[0m %s\n' "$*"; }
check() { # check <desc> <expected-rc> <actual-rc>
  if [[ "$3" -eq "$2" ]]; then pass "$1 (exit $3)"; else fail "$1 (exit $3, want $2)"; fi
}
contains() { # contains <desc> <file> <literal>
  if grep -Fq "$3" "$2"; then pass "$1"; else fail "$1: 输出中缺少 [$3]"; fi
}
sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }
assert_sha() { # assert_sha <desc> <file> <expected>
  if [[ -f "$2" ]] && [[ "$(sha_of "$2")" == "$3" ]]; then pass "$1"; else fail "$1 ($2 sha256 不符或缺失)"; fi
}

HELLO_SHA="6afe9037a89429e0572cd8fd00d2f52af4405647e88c73cb64d0436bb96f491e"
README_SHA="a1509b4ed644139a21bcb890107b4a5262eb5da26e9848c990e0e0891d0f2b52"
# 分卷 fixture 的 3 MiB 零填充 blob（内容确定，哈希稳定）
BLOB_SHA="bbd05cf6097ac9b1f89ea29d2542c1b7b67ee46848393895f5a9e43fa1f621e5"

# ---- 前置 ---------------------------------------------------------------
echo "== 准备: fixture 与 CLI"
REQUIRED_FIXTURES="sample.enc nested.enc normal.zip encrypted.7z encrypted-inner.7z masked.mp4 layered.enc split.7z.001 split.7z.001.enc split.7z.002.enc truncated.7z.001.enc"
NEED_FIXTURES=0
for f in ${REQUIRED_FIXTURES}; do [[ -f "${FIX}/${f}" ]] || NEED_FIXTURES=1; done
if [[ "${NEED_FIXTURES}" -ne 0 ]]; then
  echo "  testdata/fixtures 缺失（或缺少 v0.3.1 分卷 fixture），运行 scripts/make-fixtures.sh ..."
  "${ROOT}/scripts/make-fixtures.sh" >/dev/null || {
    echo "make-fixtures.sh 失败：e2e 需要 macOS + binaries/mac/7zz" >&2
    exit 2
  }
fi
for f in ${REQUIRED_FIXTURES}; do
  [[ -f "${FIX}/${f}" ]] || { echo "缺少 fixture ${f}" >&2; exit 2; }
done

cargo build -q -p upzip-cli --bin upzip-cli
CLI="${ROOT}/target/debug/upzip-cli"
[[ -x "${CLI}" ]] || { echo "CLI 未构建: ${CLI}" >&2; exit 2; }

run_cli() { # run_cli <args...>; captures OUT/ERR/RC
  set +e
  OUT="$("${CLI}" "$@" 2>"${TMP}/stderr.txt")"
  RC=$?
  set -e
  ERR="$(cat "${TMP}/stderr.txt")"
}
RESET_CONFIG() { rm -rf "${UPZIP_CONFIG_DIR}"; mkdir -p "${UPZIP_CONFIG_DIR}"; }

echo
echo "== 1. 基础命令"
PKG_VERSION="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "${ROOT}/Cargo.toml" | head -n1)"
run_cli --version
check "--version" 0 "${RC}"
if [[ -n "${PKG_VERSION}" && "${OUT}" == *"${PKG_VERSION}"* ]]; then pass "--version 含 ${PKG_VERSION}"; else fail "--version 不含 ${PKG_VERSION}: ${OUT}"; fi

RESET_CONFIG
run_cli doctor
check "doctor" 0 "${RC}"
if [[ "${OUT}" == *"7-Zip"* ]]; then pass "doctor 报告 7-Zip"; else fail "doctor 未报告 7-Zip"; fi

"${CLI}" --help > "${TMP}/help.txt" 2>&1
check "--help" 0 "$?"
contains "--help 列出 detect" "${TMP}/help.txt" "detect"
contains "--help 列出 cleanup-staging" "${TMP}/help.txt" "cleanup-staging"

# --licenses：v0.3.1 内嵌/随包 7-Zip 的许可证查看
run_cli --licenses
check "--licenses" 0 "${RC}"
printf '%s' "${OUT}" > "${TMP}/licenses.txt"
contains "--licenses 含 7-Zip 许可证" "${TMP}/licenses.txt" "7-Zip"
contains "--licenses 含 unRAR 限制" "${TMP}/licenses.txt" "unRAR"

echo
echo "== 2. detect / inspect（不写盘）"
run_cli detect --json "${FIX}/sample.enc"
check "detect sample.enc" 0 "${RC}"
printf '%s' "${OUT}" > "${TMP}/detect.json"
contains "detect JSON kind=ssefe_encrypted" "${TMP}/detect.json" '"kind":"ssefe_encrypted"'

run_cli detect --json "${FIX}/masked.mp4"
check "detect masked.mp4" 0 "${RC}"
printf '%s' "${OUT}" > "${TMP}/detect-masked.json"
contains "detect masked 为 masked_archive" "${TMP}/detect-masked.json" '"kind":"masked_archive"'

run_cli inspect "${FIX}/sample.enc"
check "inspect sample.enc" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/inspect.txt"
contains "inspect magic SSEFE" "${TMP}/inspect.txt" "SSEFE"
contains "inspect version 4" "${TMP}/inspect.txt" "version"
contains "inspect custom params" "${TMP}/inspect.txt" "custom params"

echo
echo "== 3. SSEFE 解密（sample.enc，密码 123）"
OUTDIR="${TMP}/out-sample"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --no-skip-processed
check "run sample.enc" 0 "${RC}"
assert_sha "sample.enc -> hello.txt 内容" "${OUTDIR}/hello.txt" "${HELLO_SHA}"
if find "${OUTDIR}" -name '.upzip-tmp-*' | grep -q .; then fail "sample.enc 残留 staging 目录"; else pass "无 staging 残留"; fi

echo
echo "== 4. 嵌套级联（nested.enc -> nested.zip -> nested-inner.zip -> hello.txt）"
OUTDIR="${TMP}/out-nested"
run_cli run "${FIX}/nested.enc" -p 123 -o "${OUTDIR}" --no-skip-processed
check "run nested.enc" 0 "${RC}"
assert_sha "级联解出 hello.txt" "${OUTDIR}/hello.txt" "${HELLO_SHA}"

echo
echo "== 5. 普通压缩包 / 加密 7z / 前缀 ZIP"
OUTDIR="${TMP}/out-normal"
run_cli run "${FIX}/normal.zip" -o "${OUTDIR}" --no-skip-processed
check "run normal.zip" 0 "${RC}"
assert_sha "normal smart layout -> normal/hello.txt" "${OUTDIR}/normal/hello.txt" "${HELLO_SHA}"
assert_sha "normal smart layout -> normal/docs/readme.txt" "${OUTDIR}/normal/docs/readme.txt" "${README_SHA}"

OUTDIR="${TMP}/out-7z"
run_cli run "${FIX}/encrypted.7z" -p 123 -o "${OUTDIR}" --no-skip-processed
check "run encrypted.7z" 0 "${RC}"
assert_sha "encrypted.7z -> encrypted/hello.txt" "${OUTDIR}/encrypted/hello.txt" "${HELLO_SHA}"

OUTDIR="${TMP}/out-masked"
run_cli run "${FIX}/masked.mp4" -o "${OUTDIR}" --no-skip-processed
check "run masked.mp4" 0 "${RC}"
assert_sha "masked.mp4 -> masked/hello.txt" "${OUTDIR}/masked/hello.txt" "${HELLO_SHA}"

echo
echo "== 6. 口令失败路径"
RESET_CONFIG
OUTDIR="${TMP}/out-wrong"
run_cli run "${FIX}/encrypted.7z" -p wrong -o "${OUTDIR}" --no-skip-processed
check "错误口令 exit 1" 1 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/wrong.txt"
if grep -FqiE '密码|password' "${TMP}/wrong.txt"; then pass "错误口令提示需要密码"; else fail "错误口令未给出密码提示"; fi
if [[ -e "${OUTDIR}/encrypted/hello.txt" ]]; then fail "错误口令却产出了文件"; else pass "错误口令无产出"; fi

OUTDIR="${TMP}/out-nopw"
run_cli run "${FIX}/sample.enc" -o "${OUTDIR}" --no-skip-processed
check "无口令 exit 1" 1 "${RC}"
if [[ -e "${OUTDIR}/hello.txt" ]]; then fail "无口令却产出了文件"; else pass "无口令无产出"; fi

echo
echo "== 6b. 逐层多密码（layered.enc：外层 outer，内层 7z 密码 inner）"
RESET_CONFIG
OUTDIR="${TMP}/out-layered"
run_cli run "${FIX}/layered.enc" -p wrong -p outer -p inner -o "${OUTDIR}" --no-skip-processed
check "layered 多密码 exit 0" 0 "${RC}"
assert_sha "layered 级联解出 hello.txt" "${OUTDIR}/hello.txt" "${HELLO_SHA}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/layered.txt"
contains "多密码成功给出候选序号警告" "${TMP}/layered.txt" "使用候选密码"

OUTDIR="${TMP}/out-layered-partial"
run_cli run "${FIX}/layered.enc" -p outer -o "${OUTDIR}" --no-skip-processed
# v0.3 设计：内层缺密码 = 部分成功（外层内容保留 + warning），不是失败。
# 这样补密码重试时不必重做外层，也不会产生重复输出。
check "只给外层密码 exit 0（内层缺密码=部分成功）" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/layered-partial.txt"
contains "内层缺密码给出保留外层警告" "${TMP}/layered-partial.txt" "保留外层内容"
if [[ -e "${OUTDIR}/encrypted-inner.7z" ]]; then pass "保留未解开的内层归档 encrypted-inner.7z"; else fail "内层归档未被保留: $(ls "${OUTDIR}" 2>/dev/null | tr '\n' ' ')"; fi
if [[ ! -e "${OUTDIR}/hello.txt" ]]; then pass "内层未解开（无 hello.txt）"; else fail "内层缺密码却解出了 hello.txt"; fi

echo
echo "== 7. --json JSON Lines"
OUTDIR="${TMP}/out-json"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --no-skip-processed --json
check "run --json" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/jsonl.txt"
contains "JSONL 含 task_detected" "${TMP}/jsonl.txt" '"type":"task_detected"'
contains "JSONL 含 task_finished" "${TMP}/jsonl.txt" '"type":"task_finished"'
LAST_LINE="$(tail -n 1 "${TMP}/jsonl.txt")"
if [[ "${LAST_LINE}" == *'"type":"summary"'* ]]; then pass "最后一行是 summary"; else fail "最后一行不是 summary: ${LAST_LINE}"; fi
if command -v python3 >/dev/null 2>&1; then
  if python3 - "${TMP}/jsonl.txt" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if line:
        json.loads(line)
PY
  then pass "所有 JSONL 行可解析"; else fail "存在无法解析的 JSONL 行"; fi
else
  echo "  SKIP python3 不可用，跳过 JSONL 逐行解析"
fi

echo
echo "== 8. --dry-run / conflict / layout"
OUTDIR="${TMP}/out-dry"
run_cli run "${FIX}/normal.zip" -o "${OUTDIR}" --dry-run --no-skip-processed
check "dry-run exit 0" 0 "${RC}"
if [[ -d "${OUTDIR}" ]] && find "${OUTDIR}" -type f | grep -q .; then fail "dry-run 写入了文件"; else pass "dry-run 未写文件"; fi

OUTDIR="${TMP}/out-rename"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --no-skip-processed
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --no-skip-processed --conflict rename
check "重复解压 rename" 0 "${RC}"
if [[ -f "${OUTDIR}/hello.txt" && -f "${OUTDIR}/hello (1).txt" ]]; then pass "conflict rename 生成 hello (1).txt"; else fail "conflict rename 未生成 hello (1).txt: $(ls "${OUTDIR}" 2>/dev/null | tr '\n' ' ')"; fi

OUTDIR="${TMP}/out-folder"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --layout folder --no-skip-processed
check "layout folder" 0 "${RC}"
assert_sha "layout folder -> sample/hello.txt" "${OUTDIR}/sample/hello.txt" "${HELLO_SHA}"

echo
echo "== 9. config / cleanup-staging"
RESET_CONFIG
run_cli config --show
check "config --show" 0 "${RC}"
run_cli config --set-password 123
check "config --set-password" 0 "${RC}"
run_cli config --show
check "config 保存后 --show" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/cfg.txt"
if grep -Fq "已保存" "${TMP}/cfg.txt"; then pass "config 显示已保存"; else fail "config 未显示已保存"; fi
run_cli config --clear-password
check "config --clear-password" 0 "${RC}"

# 密码库（v0.3.0）：多密码明文库，GUI/CLI 共用
RESET_CONFIG
run_cli config --add-password 123
check "config --add-password" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/cfg-add.txt"
contains "config --add-password 提示已添加" "${TMP}/cfg-add.txt" "已添加密码"
run_cli config --list-passwords
check "config --list-passwords" 0 "${RC}"
if [[ "${OUT}" == *"123"* ]]; then pass "config --list-passwords 输出明文"; else fail "config --list-passwords 未输出 123"; fi
run_cli config --show
check "config --show（密码库）" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/cfg-show.txt"
contains "config --show 显示密码库数量" "${TMP}/cfg-show.txt" "密码库：1 个"
if [[ -f "${UPZIP_CONFIG_DIR}/config.json" ]]; then pass "密码库文件已创建"; else fail "密码库文件未创建"; fi
if [[ "$(stat -f '%Lp' "${UPZIP_CONFIG_DIR}/config.json" 2>/dev/null)" == "600" ]]; then pass "config.json 权限 0600"; else fail "config.json 权限不是 0600"; fi

OUTDIR="${TMP}/out-lib"
run_cli run "${FIX}/sample.enc" -o "${OUTDIR}" --no-skip-processed
check "无 -p 时自动使用密码库" 0 "${RC}"
assert_sha "密码库解出 hello.txt" "${OUTDIR}/hello.txt" "${HELLO_SHA}"

run_cli config --remove-password 0
check "config --remove-password 0" 0 "${RC}"
OUT=""; ERR=""
run_cli config --list-passwords
check "config --list-passwords（已删空）" 0 "${RC}"
if [[ -z "${OUT//[[:space:]]/}" ]]; then pass "密码库已为空"; else fail "密码库未清空: ${OUT}"; fi
run_cli config --clear-passwords
check "config --clear-passwords" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/cfg-clear.txt"
contains "config --clear-passwords 提示已清空" "${TMP}/cfg-clear.txt" "已清空密码库"

STAGE_ROOT="${TMP}/stage"
mkdir -p "${STAGE_ROOT}"
touch "${STAGE_ROOT}/.upzip-tmp-abc123.zip" "${STAGE_ROOT}/.upzip-tmp-def456.zip" "${STAGE_ROOT}/keep.txt"
run_cli cleanup-staging --root "${STAGE_ROOT}"
check "cleanup-staging" 0 "${RC}"
if [[ -e "${STAGE_ROOT}/.upzip-tmp-abc123.zip" ]]; then fail "cleanup-staging 未删除残留"; else pass "cleanup-staging 删除残留"; fi
if [[ -f "${STAGE_ROOT}/keep.txt" ]]; then pass "cleanup-staging 保留非残留文件"; else fail "cleanup-staging 误删 keep.txt"; fi

echo
echo "== 10. 分卷压缩包（v0.3.1）"
RESET_CONFIG
OUTDIR="${TMP}/out-split"
run_cli run "${FIX}/split.7z.001" -o "${OUTDIR}" --no-skip-processed
check "普通分卷（只给 .001，7-Zip 自动读同目录其余卷）" 0 "${RC}"
assert_sha "普通分卷 -> split/hello.txt" "${OUTDIR}/split/hello.txt" "${HELLO_SHA}"
assert_sha "普通分卷 -> split/blob.bin" "${OUTDIR}/split/blob.bin" "${BLOB_SHA}"

# 只给非首卷：应 Skipped（exit 0）并明确提示需要第一卷，而不是 Failed。
RESET_CONFIG
ONLY2="${TMP}/only-second-volume"
mkdir -p "${ONLY2}"
cp "${FIX}/split.7z.002.enc" "${ONLY2}/"
OUTDIR="${TMP}/out-split-only2"
run_cli run "${ONLY2}/split.7z.002.enc" -o "${OUTDIR}" --no-skip-processed
check "只给 .002.enc exit 0（跳过）" 0 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/split-only2.txt"
contains "只给 .002.enc 提示需要第一卷" "${TMP}/split-only2.txt" "第一卷"
if [[ -d "${OUTDIR}" ]] && find "${OUTDIR}" -type f | grep -q .; then fail "只给 .002.enc 却产出了文件"; else pass "只给 .002.enc 无产出"; fi

# SSEFE 分卷组：每卷都是独立 .enc 容器；候选密码 wrong 在前、123 在后 -> 第 2 个候选成功。
# 只显式给 .001/.002 两个卷，其余同族卷由引擎在同目录自动发现。
RESET_CONFIG
OUTDIR="${TMP}/out-split-enc"
run_cli run "${FIX}/split.7z.001.enc" "${FIX}/split.7z.002.enc" -p wrong -p 123 -o "${OUTDIR}" --no-skip-processed --json
check "SSEFE 分卷组 exit 0" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/split-jsonl.txt"
if python3 - "${TMP}/split-jsonl.txt" <<'PY'
import json, sys
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
outcomes = [e["outcome"] for e in events if e.get("type") == "task_finished"]
summaries = [e for e in events if e.get("type") == "summary"]
assert len(summaries) == 1, summaries
assert summaries[0]["total"] == 1 and summaries[0]["success"] == 1, summaries[0]
assert len(outcomes) == 1 and outcomes[0]["files"] == 2, outcomes
assert outcomes[0]["detection"]["container"] == "7z-volume", outcomes[0]["detection"]
assert any("使用候选密码" in w for w in outcomes[0]["warnings"]), outcomes[0]["warnings"]
PY
then pass "SSEFE 分卷组：合并为 1 个任务、2 个输出、候选 #2 成功"; else fail "SSEFE 分卷组 JSON 断言失败"; fi
assert_sha "SSEFE 分卷组 -> split/hello.txt" "${OUTDIR}/split/hello.txt" "${HELLO_SHA}"
assert_sha "SSEFE 分卷组 -> split/blob.bin" "${OUTDIR}/split/blob.bin" "${BLOB_SHA}"

# 缺后续卷：必须明确报错且不留半成品。
RESET_CONFIG
OUTDIR="${TMP}/out-split-missing"
run_cli run "${FIX}/truncated.7z.001.enc" -p 123 -o "${OUTDIR}" --no-skip-processed
check "缺后续卷 exit 1" 1 "${RC}"
printf '%s\n%s\n' "${OUT}" "${ERR}" > "${TMP}/split-missing.txt"
contains "缺后续卷明确报错" "${TMP}/split-missing.txt" "缺少后续卷"
if [[ -d "${OUTDIR}" ]] && find "${OUTDIR}" -type f | grep -q .; then fail "缺后续卷却产出了半成品"; else pass "缺后续卷无半成品"; fi

echo
echo "== 12. 跳过已处理（v0.3.2：默认关闭，--skip-processed 才启用）"
# ① 默认配置连续两次 run 都应真正处理（success=1），且不生成 processed.json。
RESET_CONFIG
OUTDIR="${TMP}/out-skip-default"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --json
check "默认第 1 次 run exit 0" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/skip-default-1.jsonl"
if python3 - "${TMP}/skip-default-1.jsonl" <<'PY'
import json, sys
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
s = [e for e in events if e.get("type") == "summary"]
assert len(s) == 1 and s[0]["success"] == 1 and s[0]["skipped"] == 0, s
PY
then pass "默认第 1 次 run success=1（未跳过）"; else fail "默认第 1 次 run 未成功处理"; fi

run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --json
check "默认第 2 次 run exit 0" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/skip-default-2.jsonl"
if python3 - "${TMP}/skip-default-2.jsonl" <<'PY'
import json, sys
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
s = [e for e in events if e.get("type") == "summary"]
o = [e["outcome"] for e in events if e.get("type") == "task_finished"]
assert len(s) == 1 and s[0]["success"] == 1 and s[0]["skipped"] == 0, s
assert len(o) == 1 and o[0]["status"] == "success", o
PY
then pass "默认第 2 次 run 仍 success=1（默认不按历史跳过）"; else fail "默认第 2 次 run 被跳过"; fi
if [[ ! -e "${UPZIP_CONFIG_DIR}/processed.json" ]]; then
  pass "默认配置跑完不生成 processed.json"
else
  fail "默认配置跑完却生成了 processed.json"
fi

# ② 只有 --skip-processed 才启用指纹跳过：第 1 次成功并写记录，第 2 次 Skipped 且 reason 含「已处理过」。
RESET_CONFIG
OUTDIR="${TMP}/out-skip-optin"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --skip-processed --json
check "--skip-processed 第 1 次 exit 0" 0 "${RC}"
if [[ -f "${UPZIP_CONFIG_DIR}/processed.json" ]]; then
  pass "--skip-processed 第 1 次写入 processed.json"
else
  fail "--skip-processed 第 1 次未写入 processed.json"
fi
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --skip-processed --json
check "--skip-processed 第 2 次 exit 0" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/skip-optin-2.jsonl"
if python3 - "${TMP}/skip-optin-2.jsonl" <<'PY'
import json, sys
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
o = [e["outcome"] for e in events if e.get("type") == "task_finished"]
s = [e for e in events if e.get("type") == "summary"]
assert len(o) == 1 and o[0]["status"] == "skipped", o
assert "已处理过" in (o[0].get("reason") or ""), o[0]
assert len(s) == 1 and s[0]["skipped"] == 1 and s[0]["success"] == 0, s
PY
then pass "--skip-processed 第 2 次 Skipped 且 reason 含「已处理过」"; else fail "--skip-processed 第 2 次未按历史跳过"; fi

# ③ 分卷任务 outcome.related_paths 必须含全族卷路径（fixture 为 4 卷 .enc）。
SPLIT_VOL_COUNT="$(ls "${FIX}"/split.7z.*.enc 2>/dev/null | wc -l | tr -d ' ')"
RESET_CONFIG
OUTDIR="${TMP}/out-skip-split"
run_cli run "${FIX}/split.7z.001.enc" "${FIX}/split.7z.002.enc" -p wrong -p 123 -o "${OUTDIR}" --json
check "分卷 --json exit 0" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/skip-split.jsonl"
if python3 - "${SPLIT_VOL_COUNT}" "${TMP}/skip-split.jsonl" <<'PY'
import json, os, sys
expected = int(sys.argv[1])
events = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8") if line.strip()]
o = [e["outcome"] for e in events if e.get("type") == "task_finished"]
assert len(o) == 1, o
related = o[0].get("related_paths")
assert isinstance(related, list) and len(related) == expected, (expected, related)
assert all(isinstance(p, str) and p.endswith(".enc") for p in related), related
assert all(os.path.exists(p) for p in related), related
assert os.path.basename(related[0]).endswith("001.enc"), related
PY
then pass "分卷 related_paths 含 ${SPLIT_VOL_COUNT} 个 .enc 卷路径"; else fail "分卷 related_paths 断言失败"; fi

# ④ 清除 processed.json 后，默认 run 应重新成功处理。
rm -f "${UPZIP_CONFIG_DIR}/processed.json"
OUTDIR="${TMP}/out-skip-cleared"
run_cli run "${FIX}/sample.enc" -p 123 -o "${OUTDIR}" --json
check "清除 processed.json 后默认 run exit 0" 0 "${RC}"
printf '%s\n' "${OUT}" > "${TMP}/skip-cleared.jsonl"
if python3 - "${TMP}/skip-cleared.jsonl" <<'PY'
import json, sys
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
s = [e for e in events if e.get("type") == "summary"]
assert len(s) == 1 and s[0]["success"] == 1 and s[0]["skipped"] == 0, s
PY
then pass "清除记录后默认 run success=1"; else fail "清除记录后默认 run 未成功"; fi

echo
echo "== 11. 退出码 2（用法错误）"
run_cli run "${FIX}/normal.zip" --conflict banana
check "非法 --conflict 值 exit 2" 2 "${RC}"
run_cli run --definitely-not-a-flag
check "未知参数 exit 2" 2 "${RC}"

echo
if [[ "${FAIL}" -ne 0 ]]; then
  printf '\033[1;31m%d 项失败 / %d 项通过\033[0m\n' "${FAIL}" "${PASS}" >&2
  exit 1
fi
printf '\n\033[1;32m%d 项检查全部通过\033[0m\n' "${PASS}"
echo "ALL E2E CHECKS PASSED"
