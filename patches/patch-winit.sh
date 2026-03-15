#!/bin/bash
# Patch winit 0.30.13 for Rust 1.94+ compatibility
# (type inference regression in closure parameters for maybe_wait_on_main)
#
# Usage: Run once after `cargo fetch` or when winit source changes.
#   ./patches/patch-winit.sh

set -euo pipefail

WINIT_SRC=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -path '*/winit-0.30.13/src/window.rs' -print -quit)

if [ ! -f "$WINIT_SRC" ]; then
    echo "winit-0.30.13 source not found; run 'cargo fetch' first."
    exit 1
fi

if grep -q 'platform_impl::Window|' "$WINIT_SRC"; then
    echo "winit already patched."
    exit 0
fi

sed -i 's/maybe_wait_on_main(|w|/maybe_wait_on_main(|w: \&platform_impl::Window|/g' "$WINIT_SRC"
echo "Patched $(grep -c 'platform_impl::Window|' "$WINIT_SRC") closures in winit."
