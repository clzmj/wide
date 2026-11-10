#!/bin/sh
set -eu

MANIFEST_URL=https://wide.clzmj.dev/latest.json
REPOSITORY=https://github.com/clzmj/wide/releases/download
install_dir=${WIDE_INSTALL_DIR:-${HOME:-}/.local/bin}
dry_run=0
tmp=
stage=

usage() {
    cat <<'EOF'
Install the latest verified wide release for macOS Apple Silicon.

Usage: install.sh [--dry-run] [--help]

WIDE_INSTALL_DIR sets the user-level install directory (default: $HOME/.local/bin).
WIDE_VERSION optionally pins the manifest version. No shell startup files are edited.
EOF
}

error() {
    printf 'wide installer: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    [ -z "$stage" ] || rm -f "$stage"
    [ -z "$tmp" ] || rm -rf "$tmp"
}

valid_version() {
    v=$1
    case "$v" in
        ''|.*|*..*|*.|*[!0-9.]*) return 1 ;;
    esac
    old_ifs=$IFS
    IFS=.
    set -- $v
    IFS=$old_ifs
    [ "$#" -eq 3 ] || return 1
    for part do
        case "$part" in
            ''|*[!0-9]*|0[0-9]*) return 1 ;;
        esac
    done
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=1 ;;
        --help|-h) usage; exit 0 ;;
        *) error "unknown option: $1 (use --help)" ;;
    esac
    shift
done

[ "$(uname -s)" = Darwin ] || error "macOS is required; no files were installed."
[ "$(uname -m)" = arm64 ] || error "Apple Silicon (arm64) is required; no files were installed."
[ -n "${HOME:-}" ] || error 'HOME is not set.'
[ -n "$install_dir" ] || error 'WIDE_INSTALL_DIR must not be empty.'

for tool in curl plutil shasum tar mktemp; do
    command -v "$tool" >/dev/null 2>&1 || error "required macOS tool not found: $tool"
done

case "$install_dir" in
    /*) resolved_dir=$install_dir ;;
    *) resolved_dir=$(pwd -P)/$install_dir ;;
esac
destination=$resolved_dir/wide

trap cleanup 0
trap 'exit 1' HUP INT TERM
tmp=$(mktemp -d "${TMPDIR:-/tmp}/wide-install.XXXXXX") || error 'could not create a temporary directory.'
manifest=$tmp/latest.json
archive=$tmp/wide.tar.gz
members=$tmp/archive-members.txt
member_details=$tmp/archive-details.txt

curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 30 \
    -o "$manifest" "$MANIFEST_URL" || error 'could not download the release manifest.'
version=$(plutil -extract version raw -o - "$manifest" 2>/dev/null) \
    || error 'manifest is missing a valid version field.'
url=$(plutil -extract url raw -o - "$manifest" 2>/dev/null) \
    || error 'manifest is missing a valid url field.'
expected_sha=$(plutil -extract sha256 raw -o - "$manifest" 2>/dev/null) \
    || error 'manifest is missing a valid sha256 field.'
valid_version "$version" || error 'manifest version must be canonical numeric semver (for example 0.1.8).'
if [ -n "${WIDE_VERSION:-}" ]; then
    valid_version "$WIDE_VERSION" || error 'WIDE_VERSION must be canonical numeric semver.'
    [ "$version" = "$WIDE_VERSION" ] || error "manifest version $version does not match WIDE_VERSION=$WIDE_VERSION."
fi
case "$expected_sha" in
    *[!0123456789abcdefABCDEF]*|'') error 'manifest sha256 must contain exactly 64 hexadecimal characters.' ;;
esac
[ "${#expected_sha}" -eq 64 ] || error 'manifest sha256 must contain exactly 64 hexadecimal characters.'
expected_url=$REPOSITORY/v$version/wide-aarch64-apple-darwin.tar.gz
[ "$url" = "$expected_url" ] || error 'manifest URL does not match the expected GitHub release asset.'

curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 120 \
    -o "$archive" "$url" || error 'could not download the release archive.'
actual_sha=$(shasum -a 256 "$archive") || error 'could not compute the archive SHA-256.'
actual_sha=${actual_sha%% *}
[ "$(printf '%s' "$actual_sha" | tr 'ABCDEF' 'abcdef')" = \
  "$(printf '%s' "$expected_sha" | tr 'ABCDEF' 'abcdef')" ] \
    || error 'archive SHA-256 does not match the release manifest; existing installation was left unchanged.'

tar -tzf "$archive" > "$members" || error 'release archive is not a valid gzip tar file.'
wide_count=0
license_count=0
shell_count=0
fish_count=0
zsh_count=0
directory_count=0
while IFS= read -r member; do
    case "$member" in
        wide) wide_count=$((wide_count + 1)) ;;
        LICENSE) license_count=$((license_count + 1)) ;;
        completions/) directory_count=$((directory_count + 1)) ;;
        completions/_wide) zsh_count=$((zsh_count + 1)) ;;
        completions/wide.bash) shell_count=$((shell_count + 1)) ;;
        completions/wide.fish) fish_count=$((fish_count + 1)) ;;
        *) error "unexpected archive entry: $member" ;;
    esac
done < "$members"
[ "$wide_count" -eq 1 ] && [ "$license_count" -eq 1 ] \
    && [ "$zsh_count" -eq 1 ] && [ "$shell_count" -eq 1 ] && [ "$fish_count" -eq 1 ] \
    && [ "$directory_count" -le 1 ] \
    || error 'archive must contain wide, LICENSE, and all three shell completions exactly once.'

LC_ALL=C tar -tvzf "$archive" > "$member_details" \
    || error 'could not inspect release archive member types.'
while IFS= read -r detail; do
    [ -n "$detail" ] || continue
    kind=${detail%"${detail#?}"}
    member=${detail##* }
    case "$kind:$member" in
        -:wide|-:LICENSE|-:completions/_wide|-:completions/wide.bash|-:completions/wide.fish|d:completions/) ;;
        *) error "archive contains a non-regular or unexpected member: $detail" ;;
    esac
done < "$member_details"

mkdir "$tmp/extracted" || error 'could not prepare temporary extraction directory.'
tar -xzf "$archive" -C "$tmp/extracted" wide || error 'could not extract the wide executable.'
binary=$tmp/extracted/wide
[ -f "$binary" ] && [ ! -L "$binary" ] && [ -s "$binary" ] \
    || error 'archive wide entry is not a nonempty regular file.'
chmod 755 "$binary" || error 'could not set executable permissions.'

printf 'Verified wide %s for macOS arm64.\n' "$version"
printf 'Install destination: %s\n' "$destination"
if [ "$dry_run" -eq 1 ]; then
    printf 'Dry run: no installation files were changed.\n'
else
    mkdir -p "$resolved_dir" || error "could not create install directory: $resolved_dir"
    [ ! -d "$destination" ] || error "install destination is a directory: $destination"
    stage=$(mktemp "$resolved_dir/.wide.XXXXXX") || error 'could not create an install staging file.'
    cp "$binary" "$stage" || error 'could not stage the verified executable.'
    chmod 755 "$stage" || error 'could not set staged executable permissions.'
    mv -f "$stage" "$destination" || error 'could not atomically install wide.'
    stage=
    printf 'Installed wide %s.\n' "$version"
fi
case ":${PATH:-}:" in
    *:"$resolved_dir":*) ;;
    *) printf 'If needed, add %s to PATH manually; no shell files were edited.\n' "$resolved_dir" ;;
esac
resolved_wide=$(command -v wide 2>/dev/null || :)
if [ -n "$resolved_wide" ] && [ "$resolved_wide" != "$destination" ]; then
    printf 'Note: PATH currently resolves wide to %s; use %s or adjust PATH order manually.\n' \
        "$resolved_wide" "$destination"
fi
