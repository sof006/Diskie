#!/bin/bash
# Builds the Linux downloads from a release binary:
#   <out>/Diskie-<version>-linux-x64.tar.gz   the binary, menu entry, icon and install.sh
#   <out>/Diskie-x86_64.AppImage               one runnable file (cargo xtask release names it)
#
# usage: packaging/linux/package.sh <version> <binary> <out-dir>
set -euo pipefail

version=$1
binary=$2
out=$3
here=$(cd "$(dirname "$0")" && pwd)
id=io.github.sof006.Diskie
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# tar.gz
dir="$work/Diskie-$version-linux-x64"
mkdir -p "$dir"
install -m 755 "$binary" "$dir/diskie"
install -m 644 "$here/$id.desktop" "$here/$id.png" "$dir/"
install -m 755 "$here/install.sh" "$dir/"
tar -C "$work" -czf "$out/Diskie-$version-linux-x64.tar.gz" --owner=0 --group=0 "Diskie-$version-linux-x64"

# AppImage
app="$work/AppDir"
install -Dm 755 "$binary" "$app/usr/bin/diskie"
install -Dm 644 "$here/$id.desktop" "$app/usr/share/applications/$id.desktop"
install -Dm 644 "$here/$id.png" "$app/usr/share/icons/hicolor/256x256/apps/$id.png"
cp "$here/$id.desktop" "$here/$id.png" "$app/"
ln -s usr/bin/diskie "$app/AppRun"

# appimagetool, pinned to a release and checked against its published digest.
tool=${APPIMAGETOOL:-}
if [ -z "$tool" ]; then
    tool="$work/appimagetool"
    curl -fsSL -o "$tool" https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
    echo "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0  $tool" | sha256sum -c --quiet
    chmod +x "$tool"
fi
# No FUSE on build machines: let the tool unpack itself instead of mounting.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$app" "$out/Diskie-x86_64.AppImage"

ls -l "$out"
