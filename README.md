<img src="assets/icon.svg" width="64" height="64" alt="">

# apexide

SteelSeries GG for Linux, minus the GUI: a Rust backend that drives SteelSeries Apex
keyboards (per-key RGB, effects, OLED screen, profiles) and exposes it all through a
local HTTP/JSON API. A CLI sits on top, made for both people and agents, and a GUI can
use the same API or embed the daemon crate directly.

| Model | PID | Status |
| --- | --- | --- |
| Apex 7 TKL | `1038:1618` | tested (firmware 4.1.0) |
| Apex 7 | `1038:1612` | same protocol, untested |
| Apex Pro / Apex Pro TKL | `1038:1610` / `1614` | same protocol, untested |
| Apex 5 | `1038:161c` | same protocol, untested |

## Architecture

```
crates/
  apexide-core     hidraw I/O, packet encoders, key layout, OLED rendering (no async, no daemon)
  apexide-daemon   render thread + effects engine + HTTP API + persisted state/profiles
  apexide          the `apexide` binary: `apexide daemon` plus the CLI client
```

- **One owner.** Only the daemon's render thread talks to the keyboard. API calls change
  the desired state, and the thread renders it at about 30 fps, writing only what changed.
  It reconnects on replug and returns the keyboard to onboard lighting when it exits.
- **Layers.** Base effect, then per-key overrides, then temporary flashes, then global
  brightness. On the OLED, a temporary notification sits on top of the current mode.
- **Persistent.** State is saved to `~/.config/apexide/state.json` on every change.
  Profiles are named snapshots in `~/.config/apexide/profiles/`.
- **Transport.** HTTP on the Unix socket `$XDG_RUNTIME_DIR/apexide.sock`, with optional
  TCP (`--tcp 127.0.0.1:7171`).

## Setup

```sh
# 1. Let your user access the keyboard (no root needed afterwards)
sudo cp packaging/70-apexide.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger

# 2. Install
cargo install --path crates/apexide

# 3. Run the daemon (foreground), or as a user service
apexide daemon
cp packaging/apexide.service ~/.config/systemd/user/ && systemctl --user enable --now apexide
```

## CLI

```sh
apexide status                          # device, current lighting/OLED, profiles
apexide keys                            # key names and groups for selectors
apexide fill '#002244'                  # whole board one color
apexide set wasd red esc white          # per-key overrides (selector, color pairs)
apexide clear wasd                      # drop overrides (all if no selector)
apexide effect wave --period-ms 2500    # off static breathe spectrum wave gradient twinkle onboard
apexide effect gradient --from purple --to cyan -d down
apexide brightness 60
apexide flash fkeys red -d 2000 -b 150  # temporary blinking highlight
apexide onboard                         # hand lighting back to the keyboard
apexide frame                           # colors currently being sent

apexide oled text "hello world"         # auto-sized, word-wrapped
apexide oled image cat.gif              # PNG/JPEG/BMP/animated GIF, dithered to 1-bit
apexide oled clock -f '%H:%M:%S'
apexide oled sysinfo                    # CPU/RAM bars
apexide oled notify "Build passed" -d 3000
apexide oled preview                    # ASCII art of what the screen shows
apexide oled release                    # clear the screen and stop drawing on it

apexide profile save gaming
apexide profile apply gaming
```

**Colors:** `#rrggbb`, `#rgb`, or names (`red`, `cyan`, `warm`, `off`, ...).
**Key selectors:** comma or space separated names (`esc`, `f1`, `w`, `space`, `lshift`), raw
LED codes (`0x87`), or groups (`all letters numbers fkeys wasd arrows nav modifiers numpad`).

## For agents and scripts

The CLI is the agent interface; there is no separate MCP server or SDK to learn.

- `apexide --help` explains the lighting layers, color and key syntax, output format and
  exit codes. `apexide reference` (or `--help-verbose`) prints the full help of every
  command at once, so one read covers the whole tool. Neither needs the daemon running.
- Add `--json` to any command to get the daemon's JSON response unchanged. Errors print as
  `{"error": "..."}`.
- Exit codes: `0` success, `1` request rejected or failed, `2` invalid command line, `3` daemon
  not reachable.
- `apexide oled preview` and `apexide frame` let an agent check what it drew without seeing
  the keyboard.
- `apexide raw METHOD PATH [BODY|@FILE]` reaches any API endpoint (`apexide schema` lists them).

The API can also be called directly:

```sh
curl --unix-socket $XDG_RUNTIME_DIR/apexide.sock http://apexide/v1/status
curl --unix-socket $XDG_RUNTIME_DIR/apexide.sock http://apexide/v1/lighting/effect \
  -H 'content-type: application/json' -d '{"type":"breathe","color":"#ff4400","period_ms":4000}'
curl --unix-socket $XDG_RUNTIME_DIR/apexide.sock 'http://apexide/v1/oled/image?fit=cover' \
  --data-binary @picture.png
```

## Protocol notes

The keyboard's control channel is USB interface 1: a 64-byte output report and a 642-byte
feature report, with no report IDs.

| Command | Report | Payload |
| --- | --- | --- |
| `0x3A` direct lighting | feature | count, then `[led, r, g, b]` × count (max 160) |
| `0x3B` onboard lighting | output | — |
| `0x61` OLED | feature | 640 bytes: 128×40, row-major, MSB = leftmost pixel |
| `0x90` firmware version | output → input | ASCII reply |
| `0xFF` serial number | output → input | 19 ASCII bytes |

LED codes are HID usage codes, plus `0xF0` for the SteelSeries/Fn key. The Apex 7 TKL does
not answer the brightness read-back (`0xA3`), so apexide applies brightness in software.

Credits: protocol details come from [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB)
and [apex-tux](https://github.com/not-jan/apex-tux).
