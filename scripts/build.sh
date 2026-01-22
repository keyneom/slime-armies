#!/usr/bin/env bash
set -euo pipefail

# Unset color-related env vars that can cause "invalid value '1' for '--no-color'" with trunk
unset NO_COLOR FORCE_COLOR

trunk build --release "$@"
