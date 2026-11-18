#!/bin/sh
# Installs what building Diskie on Ubuntu or Debian needs (run as root). Used by CI as well.
set -eu
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config cmake clang curl file \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb1-dev libxcb-xkb-dev \
    libxi-dev libxrandr-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev
