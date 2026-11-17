#!/usr/bin/env bash
# What GPUI needs to build on Debian / Ubuntu (CI, the release, a Linux PC): the C toolchain, X11 and Wayland,
# xkbcommon, fonts and Vulkan headers. The .deb names the run-time libraries in Packager.toml.
set -euo pipefail
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential pkg-config \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
  libfontconfig1-dev libfreetype6-dev libvulkan-dev libzstd-dev
