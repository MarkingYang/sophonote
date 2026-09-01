#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
TARGET="$ROOT/src-tauri/target"

mkdir -p "$TARGET"
touch "$TARGET/.metadata_never_index"
