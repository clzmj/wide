#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 LABEL" >&2
  exit 2
fi

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WIDE=${WIDE:-$ROOT/target/aarch64-apple-darwin/release/wide}
mkdir -p "$ROOT/captures"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
label=$1
safe_label=$(printf '%s' "$label" | tr -c 'A-Za-z0-9._-' '_')
capture_dir=$(mktemp -d "$ROOT/captures/$stamp-$safe_label.XXXXXX")
output="$capture_dir/capture.json"
exec "$WIDE" --json advanced diagnose --label "$label" --output "$output"
