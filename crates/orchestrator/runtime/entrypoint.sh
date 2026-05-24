#!/usr/bin/env bash
set -euo pipefail
eval "$(mise activate bash --shims)"
export PATH
mise trust . || exit 10
mise install || exit 11
exec mise exec -- "$@" || exit 12
