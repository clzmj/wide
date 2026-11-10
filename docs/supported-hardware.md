# Supported hardware and validation

wide 0.1.8 targets **macOS on Apple Silicon** and the **Samsung Odyssey OLED G9 G93SC with firmware 1011.0**. Other monitor models, Intel Macs, operating systems, firmware revisions, and cable/adapter combinations are not established as supported by this validation.

## Tested setup

Live testing used **one laptop with its physical connection on HDMI1**. The monitor's HDMI1/DisplayPort source pair was exercised through its layout controls, but no second independent computer or second active feed was validated. The test therefore does not establish a working two-computer KVM, cross-computer audio routing, or behavior with two simultaneously active source machines.

Host-side DisplayPort behavior has offline planner coverage but no physical validation. USB switching was not implemented or tested. HDMI2/Micro-HDMI is outside the supported input pair. No normal GUI, daemon, or other display-control application is required.

The G93SC exposes different EDID identities and mode advertisements as layouts change. wide treats the known identities as one monitor family and rebinds using available identity/connection evidence. A matching product identity is not proof that another firmware revision behaves identically. Unrecognized profiles and unverified raw writes remain restricted.

## Resolution policy

| Role | Ordered advertised-mode preferences |
| --- | --- |
| Full-screen | 5120×1440 |
| Either equal split half | 2560×1440 |
| Big PiP/background | 5120×1440, then 3840×1080 |
| Small PiP/inset | 1920×1080, then 1280×720 |

The fallbacks reflect observed mode advertisements in this setup. They are not a promise that every firmware/input arrangement exposes those sizes. wide selects from the current macOS mode list and refuses unsupported targets; it never forces an unadvertised timing. Desktop density and refresh are preserved where available. macOS mode readback does not by itself prove physical wire timing, panel geometry, or correct proportions.

## What the matrix established

The five friendly choices produce 25 directed combinations, including five repeated-choice cases. Those combinations were exercised across interrupted runs and software fixes: **23 verified, one blocked, one partial**. The blocked case encountered a DDC read failure. In the partial case, video/resolution verification completed but the audio query timed out.

This is aggregate transition coverage, not a clean single-run pass. Firmware retains state between layouts, so one trial per friendly transition is not exhaustive coverage of every timing, cache state, PiP size/position, or prior history. Physical pane position and audible output were not independently observed for every case.

Audio confirmations came from the same single-laptop setup. E8 register values and audible confirmations support the tested picture-audio behavior; they do not prove audio follows a second laptop. The policy is main/left for split, secondary/inset for PiP, and unchanged for fullscreen. System audio output and volume are outside wide's control.

## Known reliability limit

Persistent native DDC/I²C stalls remain possible. Normal commands stop rather than automatically resetting the video link, resending settings, or rolling back. Ordinary read-only settling and NoDevice rebinding are separate from a persistent failure. A physical video-cable reconnect can be necessary; reliable automatic recovery has not been established.

The public repository omits raw hardware captures, personal display metadata, proprietary firmware, extracted code, and third-party research references. These validation limits are stated here without publishing that private investigation material.
