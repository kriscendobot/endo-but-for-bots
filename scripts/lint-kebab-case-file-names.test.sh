#!/bin/bash
# Regression test for lint-kebab-case-file-names.sh and its pattern-based
# exemptions. Builds a throwaway git repo, drops the real linter in with a
# synthetic exemptions file, and asserts the matcher's three load-bearing
# behaviors:
#
#   1. a test262-named file (an `_FIXTURE.js` under a test262 directory) is
#      exempted BY PATTERN, not by enumeration;
#   2. a non-kebab file OUTSIDE any exempt pattern is still reported;
#   3. an exact-path exemption still works (back-compat with the old exact list).
#
# Run: bash scripts/lint-kebab-case-file-names.test.sh
set -ueo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINTER="$SCRIPT_DIR/lint-kebab-case-file-names.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

git init -q
git config user.email test@example.com
git config user.name test

mkdir -p scripts
cp "$LINTER" scripts/lint-kebab-case-file-names.sh

# Synthetic exemptions exercising each pathspec form.
cat >scripts/lint-kebab-case-exemptions.txt <<'EOF'
# comment lines and blank lines are ignored

# directory prefix: the whole vendored corpus
packages/test262-runner/test262
# glob whose `*` spans `/`: the fixture-naming convention
*_FIXTURE.js
# exact path (back-compat)
packages/marshal/src/rankOrder.js
EOF

# Fixtures. Names carry a capital letter so the detector considers them.
mkdir -p packages/test262-runner/test262/harness \
  packages/somepkg/test packages/marshal/src packages/otherpkg/src
touch \
  packages/test262-runner/test262/harness/compareArray.js \
  packages/somepkg/test/some_FIXTURE.js \
  packages/marshal/src/rankOrder.js \
  packages/otherpkg/src/badCamelName.js \
  packages/otherpkg/src/good-kebab-name.js
git add -A
git commit -qm fixtures

set +e
out="$(bash scripts/lint-kebab-case-file-names.sh 2>&1)"
rc=$?
set -e

fail=0
check() { # description  expected(present|absent)  needle
  local desc="$1" mode="$2" needle="$3"
  if [ "$mode" = absent ]; then
    if printf '%s\n' "$out" | grep -qF -- "$needle"; then
      echo "FAIL: $desc -- '$needle' should NOT be reported"
      fail=1
    else
      echo "ok:   $desc"
    fi
  else
    if printf '%s\n' "$out" | grep -qF -- "$needle"; then
      echo "ok:   $desc"
    else
      echo "FAIL: $desc -- '$needle' should be reported"
      fail=1
    fi
  fi
}

echo "--- linter output (exit $rc) ---"
printf '%s\n' "$out"
echo "--------------------------------"

# 1. corpus file exempted by directory prefix
check "corpus file exempted by dir prefix" absent \
  packages/test262-runner/test262/harness/compareArray.js
# 1b. _FIXTURE outside the corpus exempted by glob
check "_FIXTURE exempted by *_FIXTURE.js glob" absent \
  packages/somepkg/test/some_FIXTURE.js
# 2. genuine non-kebab file outside every pattern still reported
check "non-exempt camelCase file reported" present \
  packages/otherpkg/src/badCamelName.js
# 3. exact-path exemption honored (back-compat)
check "exact-path exemption honored" absent \
  packages/marshal/src/rankOrder.js
# kebab-case file is never flagged (it has no capital anyway)
check "kebab-case file not reported" absent \
  packages/otherpkg/src/good-kebab-name.js

# The one genuine violation must make the linter exit non-zero.
if [ "$rc" -eq 0 ]; then
  echo "FAIL: linter exited 0 despite a non-exempt violation"
  fail=1
else
  echo "ok:   linter exited non-zero on violation (exit $rc)"
fi

if [ "$fail" -ne 0 ]; then
  echo "TESTS FAILED"
  exit 1
fi
echo "ALL TESTS PASSED"
