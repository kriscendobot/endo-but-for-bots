#!/bin/bash
# Fail when a tracked file's name is not kebab-case, unless it is exempted in
# scripts/lint-kebab-case-exemptions.txt. Wired into CI so exceptions to the
# convention are evident in CI and in PR diffs.
#
# Each exemption line is a git pathspec (default magic), so a single entry can
# name an exact path, a directory prefix (matching every file beneath it), or a
# glob in which `*` also spans `/`. That lets whole vendored trees -- notably the
# ~9.7k-file test262 corpus, which is not under our control -- be exempted with a
# couple of patterns instead of an enumerated, impossible-to-review dump.
#
# The name check itself is deliberately loose: it flags a file whose base name
# contains a lowercase letter (so it is "wordy", not a dotfile) together with a
# capital letter. A stricter checker would also reject the `_` currently
# tolerated in many names; that is left for later.
set -ueo pipefail

EXEMPTIONS=scripts/lint-kebab-case-exemptions.txt

# Turn each non-blank, non-comment exemption line into a git exclude pathspec.
exclude_pathspecs=()
while IFS= read -r pattern || [ -n "$pattern" ]; do
  case "$pattern" in
  '' | '#'*) continue ;;
  esac
  exclude_pathspecs+=(":(exclude)$pattern")
done <"$EXEMPTIONS"

# Tracked files that violate the convention and match no exemption pathspec.
# The positive `*` pathspec selects every tracked file; the excludes subtract
# the exemptions; the greps keep only wordy names bearing a capital letter.
function violators() {
  git ls-files -- '*' "${exclude_pathspecs[@]}" |
    grep '/[^/.]*[a-z][^/]*$' |
    grep '[A-Z]' |
    sort
}

# grep exits non-zero when there are no matches; tolerate that under `set -e`.
found="$(violators)" || true
if [ -n "$found" ]; then
  printf '%s\n' "$found" >&2
  echo >&2
  echo "The above file names must be kebab-case or added to $EXEMPTIONS" >&2
  exit 1
fi
