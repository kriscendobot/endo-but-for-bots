#!/bin/sh
set -eu

target=${1:?usage: cross-build.sh TARGET}
crate_directory=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workspace_directory=$(CDPATH='' cd -- "$crate_directory/../.." && pwd)

command -v cargo >/dev/null
command -v rustc >/dev/null

if command -v zig >/dev/null; then
  zig_version=$(zig version)
elif python3 -m ziglang version >/dev/null 2>&1; then
  ENDOR_ZIG_PYTHON=$(command -v python3)
  CARGO_ZIGBUILD_PYTHON_PATH=$ENDOR_ZIG_PYTHON
  export ENDOR_ZIG_PYTHON CARGO_ZIGBUILD_PYTHON_PATH
  zig_version=$($ENDOR_ZIG_PYTHON -m ziglang version)
else
  printf 'zig executable or Python ziglang package is required\n' >&2
  exit 127
fi

printf 'rustc=%s\n' "$(rustc --version)"
printf 'cargo=%s\n' "$(cargo --version)"
printf 'zig=%s\n' "$zig_version"
printf 'target=%s\n' "$target"
printf 'target_cpu=%s\n' "${RUSTFLAGS:-default}"
printf 'macos_sdk=%s\n' "${SDKROOT:-not-provisioned}"

case "$target" in
  *-unknown-linux-gnu* | *-unknown-linux-musl)
    command -v cargo-zigbuild >/dev/null
    exec cargo zigbuild --manifest-path "$workspace_directory/Cargo.toml" \
      --release --locked -p endor-git --example endor-git-link-audit \
      --target "$target"
    ;;
  x86_64-pc-windows-gnu)
    export ENDOR_ZIG_TARGET=x86_64-windows-gnu
    PATH="$crate_directory/scripts:$PATH"
    export PATH
    export CC_x86_64_pc_windows_gnu="$crate_directory/scripts/zig-cc.sh"
    export AR_x86_64_pc_windows_gnu="$crate_directory/scripts/zig-ar.sh"
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$crate_directory/scripts/zig-cc.sh"
    exec cargo build --manifest-path "$workspace_directory/Cargo.toml" \
      --release --locked -p endor-git --example endor-git-link-audit \
      --target "$target"
    ;;
  x86_64-apple-darwin | aarch64-apple-darwin)
    : "${SDKROOT:?SDKROOT must name the pinned, legally provisioned macOS SDK}"
    command -v cargo-zigbuild >/dev/null
    exec cargo zigbuild --manifest-path "$workspace_directory/Cargo.toml" \
      --release --locked -p endor-git --example endor-git-link-audit \
      --target "$target"
    ;;
  *)
    printf 'unsupported Endor Git release target: %s\n' "$target" >&2
    exit 2
    ;;
esac
