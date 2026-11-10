# Distribution

wide 0.1.8 is an early native macOS Apple Silicon CLI release. The executable is `wide`; the Rust package is also named `wide`. Original project code uses the MIT license.

## Prebuilt installation

The intended installer is:

```sh
curl -fsSL https://wide.clzmj.dev/install | sh
```

**Installer endpoint and downloadable release availability are pending verification.** This command is the planned installation interface, not a claim that publication is complete. The prebuilt path will not require Rust or Bun and defaults to `~/.local/bin`.

If necessary, add the following to your shell configuration manually:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Open a new terminal after changing your shell configuration. Verify an installed binary with `wide --version`, then review [supported hardware](supported-hardware.md) and [usage](usage.md). Installing or checking the version does not validate a monitor connection.

## Build from source

The planned public repository is `https://github.com/clzmj/wide`. Once available, source installation requires Rust and Apple command-line tools:

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
