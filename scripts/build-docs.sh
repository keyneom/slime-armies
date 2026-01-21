#!/usr/bin/env bash
set -euo pipefail

# Unset color-related env vars that can cause build issues
unset NO_COLOR FORCE_COLOR

TRUNK_BUILD_MINIFY=false trunk build --release --dist docs --public-url /slime-armies/
touch docs/.nojekyll
