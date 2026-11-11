# wide

Choose how your Mac appears on a Samsung ultrawide, from the terminal.

```sh
wide
```

The interactive form offers **Full-screen**, **Split view → Left/Right**, and **Picture in picture → Big/Small**. Left/right and big/small describe this Mac's picture. Review the preview and confirm before applying; No is selected by default and Esc cancels.

wide is an early macOS Apple Silicon CLI for the **Samsung Odyssey OLED G9 G93SC, firmware 1011.0**. Patch **0.1.9** adds recognition of the observed DisplayPort fullscreen identity and passed native read-only validation plus 102 offline checks. Release 0.1.9 and its hosted installer have been verified. It has no normal GUI, background daemon, or OpenDisplay dependency.

## Install

[Release 0.1.9](https://github.com/clzmj/wide/releases/tag/v0.1.9) is published for macOS Apple Silicon. Its hosted installer and installed version were verified over HTTPS. The prebuilt installer requires no Rust or Bun.

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

Layout and audio validation used **one laptop connected through HDMI1**. On the same G93SC connected through DisplayPort, patched 0.1.9 successfully read native status: fullscreen, DisplayPort main/HDMI1 secondary, product identity `0x749c`, and a 5120×1440 mode at 240 Hz. No setting or resolution writes were sent during that check. DisplayPort layout transitions, audio, and rebinding across layouts remain unverified. No second independent computer or second active feed was validated. Picture and audio confirmations within the single-laptop setup do not establish cross-computer switching or audio routing. USB switching was not tested.

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
