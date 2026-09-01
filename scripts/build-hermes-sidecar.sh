#!/bin/sh
set -eu

# Reproducible Hermes Runtime builder. It never copies ~/.hermes/venv:
# dependencies are resolved from the pinned uv.lock into a fresh relocatable tree.
# CPython cannot be cross-compiled: Apple targets require Darwin, Windows
# targets require a Windows host (Git Bash / CI bash).
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
VERSION=0.20.0
COMMIT=07da945c214481083049500bd29f45cabc5a04b2
SOURCE=${HERMES_SOURCE_DIR:-}
if [ -z "$SOURCE" ]; then
  echo "Set HERMES_SOURCE_DIR to a Hermes Agent ${VERSION} checkout (commit ${COMMIT})." >&2
  echo "Do not rely on a maintainer-specific absolute path." >&2
  exit 2
fi
UV_LOCK_SHA=47fe30d267657c0912c907ba443c29dd7cf21246ca922b6b665f74c2d18a6802
SOURCE_TREE_SHA=33a31bf7a6d0eb64ccb4d45051d13baba2a3cf43c1345491383c9ea976b0ab61

is_windows_host() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) return 0 ;;
  esac
  [ "${OS:-}" = "Windows_NT" ]
}

host_target() {
  if is_windows_host; then
    case "$(uname -m)" in
      x86_64|amd64|AMD64) printf '%s\n' x86_64-pc-windows-msvc ;;
      *)
        echo "Windows pack currently supports x86_64 only (host=$(uname -m))" >&2
        exit 2
        ;;
    esac
    return
  fi
  case "$(uname -s)" in
    Darwin)
      case "$(uname -m)" in
        arm64) printf '%s\n' aarch64-apple-darwin ;;
        x86_64) printf '%s\n' x86_64-apple-darwin ;;
        *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 2 ;;
      esac
      ;;
    *)
      echo "unsupported host $(uname -s)/$(uname -m); set HERMES_TARGET on Darwin or Windows" >&2
      exit 2
      ;;
  esac
}

digest_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1"
  else
    sha256sum "$1"
  fi
}

digest_hex() {
  digest_file "$1" | awk '{print $1}'
}

digest_stdin() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256
  else
    sha256sum
  fi | awk '{print $1}'
}

normalize_path() {
  printf '%s' "$1" | tr '\\' '/'
}

TARGET=${HERMES_TARGET:-$(host_target)}
case "$TARGET" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-pc-windows-msvc) ;;
  *) echo "unsupported HERMES_TARGET: $TARGET" >&2; exit 2 ;;
esac

HOST_TARGET=$(host_target)
if [ "$TARGET" != "$HOST_TARGET" ]; then
  echo "Hermes CPython cannot be cross-compiled: HERMES_TARGET=$TARGET host=$HOST_TARGET" >&2
  exit 2
fi

command -v uv >/dev/null 2>&1 || { echo "uv is required" >&2; exit 2; }
test -f "$SOURCE/pyproject.toml" || { echo "Hermes source missing: $SOURCE" >&2; exit 2; }
test "$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$SOURCE/pyproject.toml" | head -1)" = "$VERSION" || {
  echo "Hermes version does not match pinned $VERSION" >&2; exit 2;
}
test "$(digest_hex "$SOURCE/uv.lock")" = "$UV_LOCK_SHA" || {
  echo "Hermes uv.lock hash mismatch" >&2; exit 2;
}

ACTUAL_TREE_SHA=$(
  cd "$SOURCE"
  {
    find acp_adapter agent cron gateway hermes_cli plugins providers tools tui_gateway skills optional-skills -type f ! -name '.DS_Store' -print
    printf '%s\n' pyproject.toml uv.lock run_agent.py model_tools.py toolsets.py batch_runner.py trajectory_compressor.py toolset_distributions.py cli.py hermes_bootstrap.py hermes_constants.py hermes_state.py hermes_state_common.py hermes_state_portability.py hermes_state_schema.py hermes_state_search.py hermes_time.py hermes_logging.py utils.py mcp_serve.py
  } | LC_ALL=C sort | while IFS= read -r file; do digest_file "$file"; done \
    | digest_stdin
)
test "$ACTUAL_TREE_SHA" = "$SOURCE_TREE_SHA" || {
  echo "Hermes source tree hash mismatch; refusing an unreviewed checkout" >&2; exit 2;
}

OUT="$ROOT/src-tauri/resources/hermes/$TARGET"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/sophonote-hermes.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM
RUNTIME="$TMP/runtime"
mkdir -p "$RUNTIME/python" "$RUNTIME/site-packages" "$RUNTIME/bin" "$TMP/seed"

PYTHON=$(normalize_path "$(uv python find 3.11)")
case "$PYTHON" in
  [A-Za-z]:*)
    if command -v cygpath >/dev/null 2>&1; then
      PYTHON=$(cygpath -u "$PYTHON")
    fi
    ;;
esac

case "$TARGET" in
  *-pc-windows-msvc)
    PYDIR=$(CDPATH= cd -- "$(dirname -- "$PYTHON")" && pwd)
    if [ -f "$PYDIR/python.exe" ]; then
      PYTHON_ROOT=$PYDIR
      PYTHON_REL="runtime/python/python.exe"
      PYTHON_BIN="$RUNTIME/python/python.exe"
    elif [ -f "$PYDIR/bin/python.exe" ]; then
      PYTHON_ROOT=$PYDIR
      PYTHON_REL="runtime/python/bin/python.exe"
      PYTHON_BIN="$RUNTIME/python/bin/python.exe"
    else
      echo "cannot locate Windows CPython layout at $PYDIR" >&2
      exit 2
    fi
    LAUNCHER_REL="runtime/bin/hermes_watchdog.py"
    LAUNCHER_SRC="$SCRIPT_DIR/assets/hermes_watchdog.py"
    ;;
  *)
    PYTHON_ROOT=$(CDPATH= cd -- "$(dirname -- "$PYTHON")/.." && pwd)
    PYTHON_REL="runtime/python/bin/python3"
    PYTHON_BIN="$RUNTIME/python/bin/python3"
    LAUNCHER_REL="runtime/bin/hermes"
    LAUNCHER_SRC="$SCRIPT_DIR/assets/hermes-launcher"
    ;;
esac

# uv's managed CPython is self-contained; copying it avoids absolute venv symlinks.
cp -R "$PYTHON_ROOT/." "$RUNTIME/python/"

# uv-managed CPython records the builder's absolute home path in
# `_sysconfigdata_*.py`. Those values are build metadata rather than Runtime
# lookup paths, so replace the source prefix with a stable, non-personal marker
# before hashing and packaging the tree.
find "$RUNTIME/python" -type f -name '_sysconfigdata_*.py' -print | while IFS= read -r config_data; do
  "$PYTHON_BIN" - "$config_data" "$PYTHON_ROOT" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
source_prefix = sys.argv[2]
content = path.read_text(encoding="utf-8")
path.write_text(
    content.replace(source_prefix, "/opt/sophonote-build/python"),
    encoding="utf-8",
)
PY
done

# Install the locked project into an isolated target. --frozen forbids lock drift.
# SophoNote 的 Host Bridge 使用 Hermes 原生 Streamable HTTP MCP client。
# upstream 把 MCP SDK 放在锁文件内的可选 extra；未显式选择时 Runtime 虽能
# 展示 MCP 配置，却无法连接 HTTP server。这里仍完全服从同一份 uv.lock，
# 只把随 SophoNote 必需的 `mcp` extra 纳入可复现导出。
uv export --project "$SOURCE" --frozen --no-dev --extra mcp --no-emit-project --format requirements-txt --no-header --quiet --output-file "$TMP/requirements.txt"
uv pip install --python "$PYTHON_BIN" --target "$RUNTIME/site-packages" --requirements "$TMP/requirements.txt"
# Upstream deliberately refuses wheel/sdist distribution. For a signed private
# app bundle we copy exactly the setuptools-declared packages/modules instead
# of an editable install, so no .pth file or checkout path survives.
for package in acp_adapter agent cron gateway hermes_cli plugins providers tools tui_gateway; do
  cp -R "$SOURCE/$package" "$RUNTIME/site-packages/$package"
done
for module in run_agent model_tools toolsets batch_runner trajectory_compressor toolset_distributions cli hermes_bootstrap hermes_constants hermes_state hermes_state_common hermes_state_portability hermes_state_schema hermes_state_search hermes_time hermes_logging utils mcp_serve; do
  cp "$SOURCE/$module.py" "$RUNTIME/site-packages/$module.py"
done
cp -R "$SOURCE/skills" "$TMP/seed/skills"

PYTHONHOME="$RUNTIME/python" PYTHONPATH="$RUNTIME/site-packages" \
  "$PYTHON_BIN" -c \
  'from mcp.client.streamable_http import streamable_http_client' || {
  echo "bundled Hermes MCP HTTP client unavailable" >&2
  exit 2
}

# SophoNote-owned Skills are part of the client Surface, not the pinned upstream
# Hermes checkout. Overlay them into the seed after copying upstream so a
# clean-machine install receives the same Skill contract as development.
for skill in sophonote-markdown-writing sophonote-note-persistence sophonote-ai-radar sophonote-help sophonote-openrouter-rankings archify; do
  source_skill="$ROOT/skills/hermes/productivity/$skill"
  test -f "$source_skill/SKILL.md" || {
    echo "SophoNote Hermes Skill missing: $source_skill/SKILL.md" >&2
    exit 2
  }
  rm -rf "$TMP/seed/skills/productivity/$skill"
  cp -R "$source_skill" "$TMP/seed/skills/productivity/$skill"
done

# Bytecode caches are generated and mutable. Shipping them would let the first
# launch mutate both FILES.sha256 and the outer app's signed resource seal.
find "$RUNTIME" -type d -name __pycache__ -prune -exec rm -rf {} +
find "$RUNTIME" -type f \( -name '*.pyc' -o -name '*.pyo' \) -delete

cp "$LAUNCHER_SRC" "$RUNTIME/bin/$(basename "$LAUNCHER_REL")"
case "$TARGET" in
  *-pc-windows-msvc)
    cp "$SCRIPT_DIR/assets/hermes.cmd" "$RUNTIME/bin/hermes.cmd"
    ;;
  *)
    chmod 755 "$RUNTIME/bin/hermes"
    if [ -f "$RUNTIME/python/bin/python3" ]; then
      chmod 755 "$RUNTIME/python/bin/python3"
    fi
    if [ -f "$RUNTIME/python/bin/python3.11" ]; then
      chmod 755 "$RUNTIME/python/bin/python3.11"
    fi
    ;;
esac

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$RUNTIME" "$OUT/runtime"
cp -R "$TMP/seed" "$OUT/seed"

# Nested code must be signed before FILES.sha256 is produced. Signing after the
# manifest would make the Runtime's own integrity check fail; signing after the
# outer app would invalidate the outer signature. `-` is useful for local D3.
if [ "$(uname -s)" = Darwin ] && [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  find "$OUT" -type f -print0 | while IFS= read -r -d '' candidate; do
    if file "$candidate" | grep -q 'Mach-O'; then
      if [ "$APPLE_SIGNING_IDENTITY" = "-" ]; then
        codesign --force --sign - "$candidate"
      else
        codesign --force --options runtime --timestamp \
          --sign "$APPLE_SIGNING_IDENTITY" "$candidate"
      fi
    fi
  done
fi
(
  cd "$OUT"
  find runtime seed -type f ! -name FILES.sha256 -print | LC_ALL=C sort | while IFS= read -r file; do
    digest_file "$file"
  done > FILES.sha256
)
LAUNCHER_SHA=$(digest_hex "$OUT/$LAUNCHER_REL")
PYTHON_SHA=$(digest_hex "$OUT/$PYTHON_REL")
FILES_SHA=$(digest_hex "$OUT/FILES.sha256")

printf '%s\n' \
  'schema_version = 1' \
  "hermes_version = \"$VERSION\"" \
  "hermes_commit = \"$COMMIT\"" \
  "target = \"$TARGET\"" \
  "launcher = \"$LAUNCHER_REL\"" \
  "launcher_sha256 = \"$LAUNCHER_SHA\"" \
  "python = \"$PYTHON_REL\"" \
  "python_sha256 = \"$PYTHON_SHA\"" \
  'files_manifest = "FILES.sha256"' \
  "files_manifest_sha256 = \"$FILES_SHA\"" \
  "uv_lock_sha256 = \"$UV_LOCK_SHA\"" \
  "source_tree_sha256 = \"$SOURCE_TREE_SHA\"" \
  "builder = \"uv $(uv --version | awk '{print $2}')\"" \
  > "$OUT/MANIFEST.toml"

# ISSUE-041：若本脚本被沙箱进程（沙箱化的 IDE/Agent 会话）调用，写出的所有
# 文件会带 com.apple.provenance 标记；macOS AMFI 会在 exec 时把带该标记的
# linker-signed 二进制直接 SIGKILL（codesign --verify 仍通过，极难察觉）。
# xattr 不影响内容哈希，放在 FILES/MANIFEST 之后无条件清理，保证产物干净。
if [ "$(uname -s)" = Darwin ]; then
  xattr -cr "$OUT" 2>/dev/null || true
fi

case "$TARGET" in
  *-pc-windows-msvc)
    PYTHONHOME="$OUT/runtime/python" PYTHONPATH="$OUT/runtime/site-packages" \
      "$OUT/$PYTHON_REL" "$OUT/$LAUNCHER_REL" --version
    ;;
  *)
    "$OUT/runtime/bin/hermes" --version
    ;;
esac
echo "Hermes $VERSION bundled at $OUT"
