#!/bin/sh
set -eu

# 开源卫生扫描。在 SophoNote 仓库根目录执行；失败条件：出现密钥形态、
# 维护者本机绝对路径，或把用户数据库/笔记/运行时目录加入 Git 索引。
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

fail=0
tmp=$(mktemp)
filtered=$(mktemp)
trap 'rm -f "$tmp" "$filtered"' EXIT

rg -n --hidden --no-heading \
  --glob '!.git/**' \
  --glob '!scripts/oss-hygiene.sh' \
  -e 'sk-(proj-)?[a-zA-Z0-9_-]{20,}' \
  -e 'gh[pousr]_[A-Za-z0-9]{20,}' \
  -e 'github_pat_[A-Za-z0-9_]{20,}' \
  -e 'xox[bpsa]-[A-Za-z0-9-]{20,}' \
  -e 'AKIA[0-9A-Z]{16}' \
  -e 'AIza[0-9A-Za-z_-]{30,}' \
  -e 'BEGIN (RSA |OPENSSH |EC )?PRIVATE KEY' \
  . >"$tmp" || true
if [ -s "$tmp" ]; then
  echo "secret-like pattern in working tree:" >&2
  cat "$tmp" >&2
  fail=1
fi

git ls-files >"$tmp"
rg -n \
  -e '^(notes|workspace|runtime|hermes|logs)/' \
  -e '(^|/)(sophonote\.db|[^/]+\.(db|sqlite|sqlite3)(-[^/]*)?)$' \
  -e '(^|/)\.env($|\.)' \
  "$tmp" >"$filtered" || true
if [ -s "$filtered" ]; then
  rg -v '\.env\.hermes\.example$' "$filtered" >"$tmp" || true
  if [ -s "$tmp" ]; then
    echo "private runtime path tracked by git:" >&2
    cat "$tmp" >&2
    fail=1
  fi
fi

rg -n --hidden --no-heading \
  --glob '!.git/**' \
  --glob '!scripts/oss-hygiene.sh' \
  -e '/Users/fei/' \
  -e '/Users/your-name/' \
  . >"$tmp" || true
if [ -s "$tmp" ]; then
  rg -v '^(\./)?(CLAUDE\.md|AGENTS\.md):' "$tmp" >"$filtered" || true
  if [ -s "$filtered" ]; then
    echo "maintainer absolute path outside CLAUDE.md / AGENTS.md:" >&2
    cat "$filtered" >&2
    fail=1
  fi
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "oss-hygiene: ok"
