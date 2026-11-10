# wide

Choose how your Mac appears on a Samsung ultrawide, from the terminal.

```sh
wide
```

The interactive form offers **Full-screen**, **Split view → Left/Right**, and **Picture in picture → Big/Small**. Left/right and big/small describe this Mac's picture. Review the preview and confirm before applying; No is selected by default and Esc cancels.

wide **0.1.8** is an early macOS Apple Silicon release for the **Samsung Odyssey OLED G9 G93SC, firmware 1011.0**. It is a Rust CLI with no normal GUI, background daemon, or OpenDisplay dependency.

## Install

The planned prebuilt installer command is below. **Installer and release availability are pending verification.** Prebuilt installation will not require Rust or Bun.

```sh
curl -fsSL https://wide.clzmj.dev/install | sh
```

The default destination is `~/.local/bin`. If that directory is not on your PATH, add this line to your shell configuration yourself:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Source installation requires Rust and Apple command-line tools:

```sh
cargo install --git https://github.com/clzmj/wide --locked
```

See [distribution](docs/distribution.md) for release status and manual shell completions.

## What it controls

| Choice | This Mac's mode preference |
| --- | --- |
| Full-screen | 5120×1440 |
| Split left or right | 2560×1440 |
| Big PiP | 5120×1440; otherwise advertised 3840×1080 |
| Small PiP | 1920×1080; otherwise advertised 1280×720 |

wide selects only modes macOS advertises. It preserves desktop density and refresh where possible and never creates custom timings. Changes affect this Mac's mode. Monitor audio follows picture roles: main/left for split view, secondary/inset for PiP; fullscreen leaves the audio selection unchanged. System audio output and volume are not changed.

The supported input pair is **HDMI1 and DisplayPort**. Physical cable identity is kept separate from the monitor's main/secondary picture roles. An explicit port selection is needed when connection metadata is unavailable; known conflicting selections are rejected. USB switching is not implemented.

## Validation and limits

Live validation used **one laptop connected through HDMI1**. No second independent computer or second active feed was validated. Picture and audio confirmations within that setup do not establish cross-computer switching or audio routing. Physical host-side DisplayPort operation and USB switching were not tested.

All 25 directed combinations of the five friendly choices were exercised **across fixes and interrupted runs**: 23 verified, one blocked by a DDC read failure, and one partial result with video/resolution verified but an audio query timing out. This was not one clean, all-pass matrix. Register and CG mode checks do not prove every physical pane position, picture proportion, or audible result.

Persistent DDC failures remain a known limitation. Normal layout operations do not automatically reset a port, cycle a display, retry a setting, or roll back a partial transition. The reconnect guidance is:

> Unplug and reconnect this Mac’s video cable, then run wide again.

See [supported hardware](docs/supported-hardware.md) for the exact validation boundary and [usage](docs/usage.md) for commands and recovery details.

## Advanced commands

```sh
wide status
wide advanced --help
wide advanced plan split-right --port hdmi1
wide advanced apply split-right --port hdmi1 --confirm
wide advanced modes
wide --json advanced ddc read 0xE2
```

Diagnostics and explicit recovery live under `wide advanced`. Available command names do not mean every raw control is enabled; unsupported writes remain disabled.

Original code is licensed under [MIT](LICENSE). Raw captures, proprietary firmware, extracted code, and third-party research references are not part of the public distribution.
