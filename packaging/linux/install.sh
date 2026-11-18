#!/bin/sh
# Installs Diskie for the current user: the program in ~/.local/bin, plus a menu entry and icon.
# Diskie keeps itself up to date there. `./install.sh --uninstall` removes it again.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
id=io.github.sof006.Diskie

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin/diskie" "$bin/Diskie.old" "$data/applications/$id.desktop" "$data/icons/hicolor/256x256/apps/$id.png"
    echo "Diskie removed. Its settings are in ${XDG_CONFIG_HOME:-$HOME/.config}/Diskie if you want those gone too."
    exit 0
fi

mkdir -p "$bin" "$data/applications" "$data/icons/hicolor/256x256/apps"
# Through a temporary name and a rename, so a running copy is never overwritten in place.
cp "$here/diskie" "$bin/.diskie.new"
chmod 755 "$bin/.diskie.new"
mv -f "$bin/.diskie.new" "$bin/diskie"
cp "$here/$id.png" "$data/icons/hicolor/256x256/apps/$id.png"
sed "s|^Exec=.*|Exec=\"$bin/diskie\" %f|" "$here/$id.desktop" > "$data/applications/$id.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$data/applications" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$data/icons/hicolor" >/dev/null 2>&1 || true

echo "Diskie is installed: open it from your applications menu, or run $bin/diskie"
