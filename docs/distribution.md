# Distribution

wide is an early native macOS Apple Silicon CLI release. The executable is `wide`; the Rust package is also named `wide`. Original project code uses the MIT license. **0.1.9 is published**, adding recognition of the observed G93SC DisplayPort fullscreen identity `0x749c`. It passed native read-only status validation on DisplayPort at 5120×1440/240 Hz with no setting or resolution writes, plus 102 offline checks. Its hosted installer, archive checksum, ad-hoc signature, and installed version were verified. The installed form identified DisplayPort correctly and exited with no changes when Escape was pressed. This identity patch does not establish DisplayPort layout transitions, audio, rebinding across layouts, or two-computer validation.

## Prebuilt installation

The custom-domain installer is:

```sh
curl -fsSL https://wide.clzmj.dev/install | sh
```

[Release 0.1.8](https://github.com/clzmj/wide/releases/tag/v0.1.8) is published and its downloaded archive, SHA-256, ARM64 architecture, ad-hoc signature, and `--version` were checked locally. The exact hosted `curl -fsSL https://wide.clzmj.dev/install | sh` command was then verified with a temporary `WIDE_INSTALL_DIR`, downloading the real public manifest and release asset through normal TLS verification. A temporary, process-scoped curl DNS-over-HTTPS configuration bypassed the local resolver's cached earlier NXDOMAIN; no system DNS setting was changed. The installed binary matched the verified release; `--version` and `--help` passed without monitor access. The prebuilt path requires no Rust or Bun and defaults to `~/.local/bin`.

If necessary, add the following to your shell configuration manually:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Open a new terminal after changing your shell configuration. Verify an installed binary with `wide --version`, then review [supported hardware](supported-hardware.md) and [usage](usage.md). Installing or checking the version does not validate a monitor connection.

## Build from source

The public repository is [clzmj/wide](https://github.com/clzmj/wide). Source installation requires Rust and Apple command-line tools:

```sh
cargo install --git https://github.com/clzmj/wide --locked
```

Cargo normally installs the executable in `~/.cargo/bin`. A prebuilt download does not require the Rust toolchain. Source code compiling successfully on another platform does not establish native display-control support there.

## Manual completions

Completion files are provided in the repository's `completions` directory. The commands below assume a source checkout as the current directory; no installer modification of shell configuration is implied.

For Bash, load the completion file in the current shell:

```sh
source ./completions/wide.bash
```

For persistent Bash use, copy that file to a chosen stable location and add a `source` line to your Bash configuration manually.

For Zsh:

```sh
mkdir -p ~/.zsh/completions
cp ./completions/_wide ~/.zsh/completions/_wide
```

Add these lines to `.zshrc` before any existing completion initialization, adapting an existing setup rather than duplicating it:

```sh
fpath=("$HOME/.zsh/completions" $fpath)
autoload -Uz compinit
compinit
```

For Fish:

```sh
mkdir -p ~/.config/fish/completions
cp ./completions/wide.fish ~/.config/fish/completions/wide.fish
```

## Public package boundary

The public source/release package should contain the CLI, build metadata, license, completion files, and public documentation. Raw captures, identifying hardware metadata, proprietary firmware, extracted code, and third-party research references remain outside the public distribution. No Rust/Bun runtime, OpenDisplay application, normal GUI, or background daemon is required by the prebuilt executable.

## Maintaining a release

[Check](../.github/workflows/check.yml) runs on pull requests, pushes to `main`, and calls from [Release](../.github/workflows/release.yml). It verifies a native ARM64 macOS runner, then checks Rust formatting, Clippy with warnings denied, Rust tests, offline Python fixtures (including installer tests), and shell/completion syntax. No monitor is needed. Run the local checks before tagging:

```sh
bash diagnostics/checks.sh
# Installer tests alone, with local fixtures:
uv run --script tests/test_install.py
```

The local check script also builds the optimized ARM64 executable and writes local validation logs. These are software checks, not hardware qualification.

Release runs for pushed `v*` tags and depends on Check succeeding. The tag must exactly match `v` plus the version in `Cargo.toml`, with a numeric `major.minor.patch` version. The release job builds the default-feature ARM64 binary and invokes [package-release.sh](../scripts/package-release.sh). Packaging verifies native architecture and binary version, ad-hoc signs and verifies the copied executable, and allowlists only the binary, MIT license, and completions. Ad-hoc signing is not notarization.

The GitHub release receives `wide-aarch64-apple-darwin.tar.gz` and its `.sha256` checksum file. Only after release success does the Pages job deploy the installer and `latest.json`, containing that release's version, download URL, and SHA-256. The site declares `wide.clzmj.dev` as its custom domain. Workflow configuration alone does not establish that the endpoint or a particular release is live; verify both after deployment.

Publishing uses GitHub's built-in repository-scoped token with `contents: write` only in the release job. Pages uses its own job permissions (`contents: read`, `pages: write`, `id-token: write`) and GitHub's Pages/OIDC deployment flow. The workflow does not use a shared Cloudflare token.

For a future patch, update `Cargo.toml` and `Cargo.lock`, run the checks, commit and push the reviewed changes, then tag that commit. For example, after setting version 0.1.10:

```sh
git tag v0.1.10
git push origin v0.1.10
```

Review the Check, Release, and Pages results, download and verify the asset checksum, and test the published installer before announcing availability. This release process does not run the hardware matrix.


## Initial hosting setup

GitHub Pages is enabled with `build_type: workflow` and custom domain `wide.clzmj.dev`. Its `github-pages` environment allows `main` and release tags matching `v*`; GitHub initially allowed only `main`, which rejected the first tag deployment before a runner started. Adding the tag policy and rerunning only the failed Pages job succeeded without republishing the release asset.

DNS must have a DNS-only CNAME `wide` pointing to `clzmj.github.io`. After the DNS check and certificate issuance succeed, enable HTTPS enforcement in the repository's Pages settings. Then check the actual endpoint without bypassing TLS verification:

```sh
curl -fsSL https://wide.clzmj.dev/install -o /tmp/wide-install.sh
sh /tmp/wide-install.sh --dry-run
```

The custom-domain installer was verified live on 2026-11-09, with HTTPS enforcement enabled. The authenticated Cloudflare operator CLI created the single DNS record; no other project's Cloudflare credentials were used. GitHub initially cached the missing DNS result. Removing and readding the custom domain once after DNS propagation restarted certificate provisioning, following [GitHub's documented recovery](https://docs.github.com/en/pages/getting-started-with-github-pages/securing-your-github-pages-site-with-https#troubleshooting-certificate-provisioning-certificate-not-yet-created-error). The local resolver still retained an earlier NXDOMAIN result at verification time; public DNS and GitHub validated the record. This transient negative-cache result must expire before the unmodified command works through that resolver. Raw investigation files and Cloudflare CLI account caches remain ignored in the canonical local repository; they were not uploaded to GitHub.
