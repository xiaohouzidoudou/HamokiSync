#!/bin/sh
# User-local installation, no sudo and no credentials read.
set -eu
src=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
dest="$HOME/.local/share/hamokisync"
mkdir -p "$dest" "$HOME/.local/share/applications"
install -m 755 "$src/hamokisync-settings" "$src/hamokisync-daemon" "$dest/"
install -m 644 "$src/hamokisync.png" "$dest/"
case "$dest" in *\"*|*%*|*\$*|*\`*) echo "Unsupported install path" >&2; exit 1;; esac
cat > "$HOME/.local/share/applications/hamokisync.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=HamokiSync
Exec="$dest/hamokisync-settings"
Icon=$dest/hamokisync.png
Terminal=false
Categories=Utility;
EOF
printf '%s\n' "Installed. Open HamokiSync from your application menu and configure your own server."
