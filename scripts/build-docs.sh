#!/usr/bin/env bash
set -euo pipefail

TRUNK_BUILD_MINIFY=false trunk build --release --dist docs --public-url /slime-armies/
touch docs/.nojekyll
