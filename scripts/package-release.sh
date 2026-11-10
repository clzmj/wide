#!/bin/bash
# Package only a built binary and the explicitly public runtime files. No monitor queries.
set -euo pipefail
umask 022
cd "$(dirname "$0")/.."

binary=${1:-target/aarch64-apple-darwin/release/wide}
output=${2:-dist}
archive_name=wide-aarch64-apple-darwin.tar.gz
version=$(awk '/^\[package\]$/{package=1;next} /^\[/{package=0} package && $1=="version" {gsub(/"/,"",$3);print $3;exit}' Cargo.toml)
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'Invalid package version' >&2; exit 1; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Packaging requires native ARM64 macOS' >&2; exit 1; }
[[ -f "$binary" && -x "$binary" ]] || { echo 'Built wide binary is missing' >&2; exit 1; }
[[ "$(lipo -archs "$binary")" == arm64 ]] || { echo 'Expected an ARM64-only Mach-O binary' >&2; exit 1; }
mkdir -p "$output"
output=$(cd "$output" && pwd)
[[ ! -e "$output/$archive_name" && ! -e "$output/$archive_name.sha256" ]] || { echo 'Release archive already exists; choose a fresh output directory' >&2; exit 1; }
stage=$(mktemp -d "${TMPDIR:-/tmp}/wide-release.XXXXXX")
trap 'rm -rf "$stage"' EXIT
install -m 755 "$binary" "$stage/wide"
install -m 644 LICENSE "$stage/LICENSE"
mkdir "$stage/completions"
for file in _wide wide.bash wide.fish; do
  install -m 644 "completions/$file" "$stage/completions/$file"
done
# Ad-hoc sign our copied executable; this does not notarize it or alter host protections.
codesign --force --sign - --timestamp=none "$stage/wide"
codesign --verify --strict --verbose=2 "$stage/wide"
[[ "$("$stage/wide" --version)" == "wide $version" ]] || { echo 'Binary and Cargo versions differ' >&2; exit 1; }
COPYFILE_DISABLE=1 tar -czf "$output/$archive_name" -C "$stage" wide LICENSE completions
(cd "$output" && shasum -a 256 "$archive_name" > "$archive_name.sha256")
printf 'Archive: %s\n' "$output/$archive_name"
cat "$output/$archive_name.sha256"
