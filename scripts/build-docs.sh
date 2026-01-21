#!/usr/bin/env bash
set -euo pipefail

TRUNK_BUILD_MINIFY=false trunk build --release --dist docs
touch docs/.nojekyll
