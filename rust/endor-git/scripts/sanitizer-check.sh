#!/bin/sh
set -eu

crate_directory=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workspace_directory=$(CDPATH='' cd -- "$crate_directory/../.." && pwd)
target_directory="$workspace_directory/target/endor-git-address-sanitizer"
address_sanitizer_runtime=$(gcc -print-file-name=libasan.so)

test -f "$address_sanitizer_runtime"
CC=gcc \
  CFLAGS='-fsanitize=address -fno-omit-frame-pointer' \
  RUSTFLAGS="-C link-arg=$address_sanitizer_runtime" \
  CARGO_TARGET_DIR="$target_directory" \
  cargo test --manifest-path "$workspace_directory/Cargo.toml" --locked \
    -p endor-git --test conformance --no-run

test_binary=$(find "$target_directory/debug/deps" -maxdepth 1 -type f \
  -name 'conformance-*' -perm -u+x -print -quit)
test -n "$test_binary"
LD_PRELOAD="$address_sanitizer_runtime" "$test_binary"
