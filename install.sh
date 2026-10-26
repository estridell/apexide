#!/bin/sh
# Install the latest apexide release.
#
#   curl -fsSL https://raw.githubusercontent.com/estridell/apexide/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --no-udev --no-service
#
# Puts the binary in ~/.local/bin (or $APEXIDE_BIN_DIR), installs the udev
# rule (asks for sudo) and enables the apexide systemd user service. Running
# it again reinstalls the latest release; `apexide update` does the same for
# the binary alone.
set -eu

repo=estridell/apexide
base="https://github.com/$repo/releases/latest/download"
bin_dir="${APEXIDE_BIN_DIR:-$HOME/.local/bin}"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
udev=1
service=1

for arg in "$@"; do
    case "$arg" in
        --no-udev) udev=0 ;;
        --no-service) service=0 ;;
        -h | --help)
            echo "usage: install.sh [--no-udev] [--no-service]  (env: APEXIDE_BIN_DIR)"
            exit 0
            ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "apexide only runs on Linux"
arch="$(uname -m)"
asset="apexide-$arch-linux"
command -v curl >/dev/null || die "curl is required"
command -v sha256sum >/dev/null || die "sha256sum is required"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "downloading $asset"
curl -fsSL -o "$tmp/$asset" "$base/$asset" ||
    die "no prebuilt binary for $arch; build from source instead (see the README)"
curl -fsSL -o "$tmp/$asset.sha256" "$base/$asset.sha256"
(cd "$tmp" && sha256sum -c --quiet "$asset.sha256") || die "checksum mismatch"

mkdir -p "$bin_dir"
install -m 755 "$tmp/$asset" "$bin_dir/apexide"
say "installed $("$bin_dir/apexide" --version) to $bin_dir/apexide"

if [ "$udev" = 1 ]; then
    say "installing the udev rule (needs sudo)"
    curl -fsSL -o "$tmp/70-apexide.rules" "$base/70-apexide.rules"
    sudo install -m 644 "$tmp/70-apexide.rules" /etc/udev/rules.d/70-apexide.rules
    sudo udevadm control --reload
    sudo udevadm trigger
fi

if [ "$service" = 1 ]; then
    say "enabling the apexide user service"
    curl -fsSL -o "$tmp/apexide.service" "$base/apexide.service"
    sed -i "s|^ExecStart=.*|ExecStart=$bin_dir/apexide daemon|" "$tmp/apexide.service"
    mkdir -p "$unit_dir"
    install -m 644 "$tmp/apexide.service" "$unit_dir/apexide.service"
    systemctl --user daemon-reload
    systemctl --user enable apexide.service
    systemctl --user restart apexide.service
fi

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) say "note: $bin_dir is not on your PATH" ;;
esac
say "done. Try: apexide status"
