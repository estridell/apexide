<img src="assets/icon.svg" width="64" height="64" alt="">

# apexkb-rs

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
  apexkb-core     hidraw I/O, packet encoders, key layout, OLED rendering (no async, no daemon)
  apexkb-daemon   render thread + effects engine + HTTP API + persisted state/profiles
  apexkb          the `apexkb` binary: `apexkb daemon` plus the CLI client
```

- **One owner.** Only the daemon's render thread talks to the keyboard. API calls change
  the desired state, and the thread renders it at about 30 fps while something animates
  (and only when something is due otherwise), writing only what changed.
  It reconnects on replug and returns the keyboard to onboard lighting when it exits.
- **Layers.** Base effect, then per-key overrides, then temporary flashes, then global
  brightness. On the OLED, a temporary notification sits on top of the current mode.
- **Persistent.** Lighting is saved to `~/.config/apexkb/state.json` and the OLED mode to
  `oled.json` next to it, each whenever it changes.
  Profiles are named snapshots in `~/.config/apexkb/profiles/`.
- **Transport.** HTTP on the Unix socket `$XDG_RUNTIME_DIR/apexkb.sock`, with optional
  TCP (`--tcp 127.0.0.1:7171`).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/estridell/apexkb-rs/main/install.sh | sh
```

This downloads the latest release binary (static, x86_64) to `~/.local/bin/apexkb`,
installs the udev rule so your user can access the keyboard (asks for sudo), and
enables the `apexkb` systemd user service. Skip parts with
`| sh -s -- --no-udev --no-service`, or pick another directory with `APEXKB_BIN_DIR`.

```sh
apexkb update --check                  # is there a newer release?
apexkb update                          # install it and restart the service
```

### From source

```sh
# 1. Let your user access the keyboard (no root needed afterwards)
sudo cp packaging/70-apexkb.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger

# 2. Install to ~/.local/bin
cargo install --path crates/apexkb --root ~/.local

# 3. Run the daemon (foreground), or as a user service
apexkb daemon
cp packaging/apexkb.service ~/.config/systemd/user/ && systemctl --user enable --now apexkb
```

Source builds report a `-dev` version (`0.1.0-dev`), which `apexkb update` treats as
older than any release on that line.

### Releases

Every merge to `main` publishes a GitHub release. Versions are `MAJOR.MINOR.N`:
`MAJOR.MINOR` comes from the workspace version in `Cargo.toml` and `N` counts releases on
that line. To start a new line, change `0.1.0` to `0.2.0` in `Cargo.toml` in a PR; the
merge releases `v0.2.0`, and the next one `v0.2.1`.

## CLI

```sh
apexkb status                          # device, current lighting/OLED, profiles
apexkb keys                            # key names and groups for selectors
apexkb fill '#002244'                  # whole board one color
apexkb set wasd red esc white          # per-key overrides (selector, color pairs)
apexkb clear wasd                      # drop overrides (all if no selector)
apexkb effect wave --period-ms 2500    # off static breathe spectrum wave gradient twinkle onboard
apexkb effect gradient --from purple --to cyan -d down
apexkb brightness 60
apexkb flash fkeys red -d 2000 -b 150  # temporary blinking highlight
apexkb onboard                         # hand lighting back to the keyboard
apexkb frame                           # colors currently being sent

apexkb oled text "hello world"         # auto-sized, word-wrapped
apexkb oled image cat.gif              # PNG/JPEG/BMP/animated GIF, dithered to 1-bit
apexkb oled clock -f '%H:%M:%S'
apexkb oled sysinfo                    # CPU/RAM bars (default)
apexkb oled notify "Build passed" -d 3000
apexkb oled preview                    # ASCII art of what the screen shows
apexkb oled release                    # clear the screen and stop drawing on it

apexkb profile save gaming
apexkb profile apply gaming
```

**Colors:** `#rrggbb`, `#rgb`, or names (`red`, `cyan`, `warm`, `off`, ...).
**Key selectors:** comma or space separated names (`esc`, `f1`, `w`, `space`, `lshift`), raw
LED codes (`0x87`), or groups (`all letters numbers fkeys wasd arrows nav modifiers numpad`).

## For agents and scripts

The CLI is the agent interface; there is no separate MCP server or SDK to learn.

- `apexkb --help` explains the lighting layers, color and key syntax, output format and
  exit codes. `apexkb reference` (or `--help-verbose`) prints the full help of every
  command at once, so one read covers the whole tool. Neither needs the daemon running.
- Add `--json` to any command to get the daemon's JSON response unchanged. Errors print as
  `{"error": "..."}`.
- Exit codes: `0` success, `1` request rejected or failed, `2` invalid command line, `3` daemon
  not reachable.
- `apexkb oled preview` and `apexkb frame` let an agent check what it drew without seeing
  the keyboard.
- `apexkb raw METHOD PATH [BODY|@FILE]` reaches any API endpoint (`apexkb schema` lists them).

The API can also be called directly:

```sh
curl --unix-socket $XDG_RUNTIME_DIR/apexkb.sock http://apexkb/v1/status
curl --unix-socket $XDG_RUNTIME_DIR/apexkb.sock http://apexkb/v1/lighting/effect \
  -H 'content-type: application/json' -d '{"type":"breathe","color":"#ff4400","period_ms":4000}'
curl --unix-socket $XDG_RUNTIME_DIR/apexkb.sock 'http://apexkb/v1/oled/image?fit=cover' \
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

A `0x3A` packet only changes the keys it lists: on top of onboard lighting, every other key
keeps its onboard color (verified on the Apex 7 TKL). A single key cannot be handed back to
onboard lighting; `0x3B` restores the whole board.

Holding the OLED menu button opens the firmware's OLED menu. While it is open the keyboard
ignores `0x61` frames, and when it closes the menu's last frame stays until the next `0x61`.
Neither event is reported: interface 4 (vendor page `0xFFC1`, 64-byte input reports) only
sends `02 01 <level>` when the roller changes the illumination brightness in the menu. So the
daemon rewrites the OLED every 250 ms, which clears the menu frame right after it closes
(verified on the Apex 7 TKL). `release` mode never draws, so the menu frame stays there.

LED codes are HID usage codes, plus `0xF0` for the SteelSeries/Fn key. The Apex 7 TKL does
not answer the brightness read-back (`0xA3`), so apexkb applies brightness in software.

Credits: protocol details come from [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB)
and [apex-tux](https://github.com/not-jan/apex-tux).
