#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WIDE=${WIDE:-$ROOT/target/aarch64-apple-darwin/release/wide}
exec "$WIDE" --json advanced ddc scan --read-only "$@"
