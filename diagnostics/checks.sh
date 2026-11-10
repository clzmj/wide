#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p captures/validation
cargo fmt --all --check
cargo clippy --locked --all-targets --target aarch64-apple-darwin -- -D warnings 2>&1 | tee captures/validation/clippy-final.txt
cargo test --locked --target aarch64-apple-darwin 2>&1 | tee captures/validation/rust-tests-final.txt
uv run --script tests/test_compare.py 2>&1 | tee captures/validation/compare-tests-final.txt
uv run --script tests/test_matrix.py 2>&1 | tee captures/validation/matrix-tests-final.txt
uv run --script tests/test_install.py 2>&1 | tee captures/validation/installer-tests-final.txt
sh -n install.sh
bash -n scripts/package-release.sh
for script in diagnostics/*.sh completions/wide.bash; do bash -n "$script"; done
zsh -n completions/_wide
cargo build --release --locked --target aarch64-apple-darwin
file target/aarch64-apple-darwin/release/wide > captures/validation/binary-final.txt
shasum -a 256 target/aarch64-apple-darwin/release/wide > captures/validation/binary-sha256.txt
echo 'Quality checks and optimized build passed'
