#!/bin/sh
set -eu

if test -n "${ENDOR_ZIG_PYTHON:-}"; then
  exec "$ENDOR_ZIG_PYTHON" -m ziglang ar "$@"
fi
exec zig ar "$@"
