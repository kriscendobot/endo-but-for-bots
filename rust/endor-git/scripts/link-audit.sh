#!/bin/sh
set -eu

artifact=${1:?usage: link-audit.sh ARTIFACT}
test -f "$artifact"

case "$(uname -s)" in
  Linux)
    dependencies=$(ldd "$artifact" 2>&1 || true)
    ;;
  Darwin)
    dependencies=$(otool -L "$artifact")
    ;;
  MINGW* | MSYS* | CYGWIN*)
    dependencies=$(objdump -p "$artifact" | sed -n 's/^[[:space:]]*DLL Name: /DLL /p')
    ;;
  *)
    printf 'no linkage auditor for this host\n' >&2
    exit 2
    ;;
esac

printf '%s\n' "$dependencies"
if printf '%s\n' "$dependencies" | grep -Eiq \
  'libgit2|libssl|libcrypto|libssh2|libcurl|libz\.so|zlib[0-9]*\.dll'; then
  printf 'unexpected dynamic dependency in Endor Git artifact\n' >&2
  exit 1
fi
