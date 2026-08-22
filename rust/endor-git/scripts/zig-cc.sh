#!/usr/bin/env bash
set -euo pipefail

: "${ENDOR_ZIG_TARGET:?ENDOR_ZIG_TARGET must name a Zig compilation target}"
arguments=()
for argument in "$@"; do
  case "$argument" in
    --target=*) ;;
    *) arguments+=("$argument") ;;
  esac
done
if test -n "${ENDOR_ZIG_PYTHON:-}"; then
  exec "$ENDOR_ZIG_PYTHON" -m ziglang cc -target "$ENDOR_ZIG_TARGET" "${arguments[@]}"
fi
exec zig cc -target "$ENDOR_ZIG_TARGET" "${arguments[@]}"
