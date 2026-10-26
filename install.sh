#!/bin/sh
# Install the latest apexkb release.
#
#   curl -fsSL https://raw.githubusercontent.com/estridell/apexkb-rs/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --no-udev --no-service
#
# Puts the binary in ~/.local/bin (or $APEXKB_BIN_DIR), installs the udev
# rule (asks for sudo) and enables the apexkb systemd user service. Running
# it again reinstalls the latest release; `apexkb update` does the same for
# the binary alone.
set -eu

repo=estridell/apexkb-rs
bin_dir="${APEXKB_BIN_DIR:-$HOME/.local/bin}"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
udev=1
service=1

for arg in "$@"; do
    case "$arg" in
        --no-udev) udev=0 ;;
        --no-service) service=0 ;;
        -h | --help)
            echo "usage: install.sh [--no-udev] [--no-service]  (env: APEXKB_BIN_DIR)"
            exit 0
            ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "apexkb only runs on Linux"
arch="$(uname -m)"
asset="apexkb-$arch-linux"
command -v curl >/dev/null || die "curl is required"
command -v sha256sum >/dev/null || die "sha256sum is required"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The highest version, not GitHub's "Latest" flag, which can lag when release
# runs finish out of order. Prerelease tags (with a '-') are skipped.
tag=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=100" |
    grep -o '"tag_name": *"v[0-9.]*"' | grep -o 'v[0-9.]*' | sort -V | tail -1)
[ -n "$tag" ] || die "no published releases found"
base="https://github.com/$repo/releases/download/$tag"

say "downloading $asset $tag"
curl -fsSL -o "$tmp/$asset" "$base/$asset" ||
    die "no prebuilt binary for $arch; build from source instead (see the README)"
curl -fsSL -o "$tmp/$asset.sha256" "$base/$asset.sha256"
(cd "$tmp" && sha256sum -c --quiet "$asset.sha256") || die "checksum mismatch"

mkdir -p "$bin_dir"
install -m 755 "$tmp/$asset" "$bin_dir/apexkb"
say "installed $("$bin_dir/apexkb" --version) to $bin_dir/apexkb"

if [ "$udev" = 1 ]; then
    say "installing the udev rule (needs sudo)"
    curl -fsSL -o "$tmp/70-apexkb.rules" "$base/70-apexkb.rules"
    sudo install -m 644 "$tmp/70-apexkb.rules" /etc/udev/rules.d/70-apexkb.rules
    sudo udevadm control --reload
    sudo udevadm trigger
fi

if [ "$service" = 1 ]; then
    say "enabling the apexkb user service"
    curl -fsSL -o "$tmp/apexkb.service" "$base/apexkb.service"
    sed -i "s|^ExecStart=.*|ExecStart=$bin_dir/apexkb daemon|" "$tmp/apexkb.service"
    mkdir -p "$unit_dir"
    install -m 644 "$tmp/apexkb.service" "$unit_dir/apexkb.service"
    systemctl --user daemon-reload
    systemctl --user enable apexkb.service
    systemctl --user restart apexkb.service
fi

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) say "note: $bin_dir is not on your PATH" ;;
esac
say "done. Try: apexkb status"
