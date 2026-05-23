#!/usr/bin/env bash
set -euo pipefail
eval "$(mise activate bash --shims)"
mise trust . || exit 10
mise install || exit 11
exec "$@" || exit 12
