# Using wide

Run `wide` in an interactive terminal. Choose Full-screen, Split view left/right, or Picture in picture big/small, then review the arrangement, this Mac's resolution, and monitor-audio policy. Apply defaults to No; Esc cancels.

The five choices describe this Mac's role. They do not imply a second computer has been detected or configured. wide runs on macOS Apple Silicon and adjusts only the local Mac's display mode. See [supported hardware](supported-hardware.md) before applying a layout.

## Status and direct application

```sh
wide status
wide advanced list
wide advanced info
wide advanced plan pip-small --port hdmi1
wide advanced apply pip-small --port hdmi1 --confirm
```

Accepted layout names are `fullscreen`, `split-left`, `split-right`, `pip-big`, and `pip-small`. `plan` previews without changing settings; `apply` requires `--confirm`.

The supported physical port names are `hdmi1` and `dp`. Connection metadata may identify the cable; when it does not, choose the actual port yourself. `--port` does not move a cable or override a detected conflict. The global `--display <name-or-registry-id>` selector can identify one connection when discovery is ambiguous. Registry IDs can change when the monitor reconnects.

## Modes and audio

```sh
wide advanced modes
wide advanced sound
wide advanced sound --confirm
```

`modes` lists advertised macOS modes. Resolution selection prefers 5120×1440 for fullscreen, 2560×1440 for equal split, 5120×1440 then 3840×1080 for big PiP, and 1920×1080 then 1280×720 for small PiP. Unsupported sizes are refused rather than synthesized. Mode pixels and register readback are software observations; inspect the physical picture if proportions look wrong.

`sound` previews the policy for the current layout; `--confirm` applies audio only. Split view selects main/left picture audio, PiP selects secondary/inset picture audio, and fullscreen leaves the selection unchanged. These are monitor picture roles, not system-output-device settings. Audio write readback does not establish audible cross-computer routing.

## Diagnostics

```sh
wide --json status
wide --json advanced ddc read 0xE2
wide --json advanced ddc read 0xE3
wide --json advanced ddc read 0xE8
wide advanced diagnose --output ./wide-diagnostic.json --label current-layout
wide advanced ddc scan --read-only
```

Get VCP operations send read requests on the monitor's DDC channel. They can still encounter driver timeouts. Use targeted reads before a broad scan. Diagnostic files may contain identifying display metadata; review them before sharing.

Video settings are attempted once per planned step. Read-only settling and ordinary NoDevice rebinding may occur, but a failed setting or persistent transport failure stops the sequence. A stopped operation can leave a partially changed layout; there is no automatic rollback.

## Reconnect guidance

Persistent DDC stalls have occurred on the tested setup. The normal error message is:

> Unplug and reconnect this Mac’s video cable, then run wide again.

This is a manual cable action. wide does not automatically reset a port. Explicit advanced software attempts are available:

```sh
wide advanced reconnect
wide advanced reconnect --confirm
wide advanced reconnect --layout split-left
wide advanced reconnect --layout split-left --confirm
wide advanced reconnect --cycle
wide advanced reconnect --cycle --confirm
```

Without `--confirm`, these commands preview. A confirmed reconnect without `--layout` reapplies the current advertised mode; an explicit layout chooses its resolution policy without changing monitor layout registers. `--cycle` attempts a private macOS logical deactivate/reactivate operation and requires another active display. It is separate from normal layout application. Neither mode reapplication nor logical cycling is established as a reliable replacement for physical reconnection, and neither guarantees restored DDC communication.

Use `wide advanced --help` and the relevant subcommand's `--help` for the installed version. Some legacy raw-write command names remain visible but reject unverified operations.
