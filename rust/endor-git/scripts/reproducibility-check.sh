#!/bin/sh
set -eu

crate_directory=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workspace_directory=$(CDPATH='' cd -- "$crate_directory/../.." && pwd)
comparison_parent=${GARDEN_SCRATCH:-"$workspace_directory/target"}
mkdir -p "$comparison_parent"
comparison_directory=$(mktemp -d "$comparison_parent/endor-git-repro.XXXXXX")
trap 'rm -rf "$comparison_directory"' EXIT HUP INT TERM

for build_number in 1 2; do
  target_directory="$comparison_directory/target-$build_number"
  CARGO_TARGET_DIR="$target_directory" cargo build \
    --manifest-path "$workspace_directory/Cargo.toml" --release --locked \
    -p endor-git --example endor-git-link-audit
  cp "$target_directory/release/examples/endor-git-link-audit" \
    "$comparison_directory/artifact-$build_number"
  strip -g "$comparison_directory/artifact-$build_number"
done

cmp "$comparison_directory/artifact-1" "$comparison_directory/artifact-2"
printf 'reproducibility check passed\n'
