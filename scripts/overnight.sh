#!/usr/bin/env bash
# Train a fresh brain, then score it on utterances it has never seen.
#
# The order is the whole point. Teaching is on for the curriculum and the
# training half, off for the test half, so the final number is what Spoon can
# do rather than what it has memorized. A baseline is measured first on an
# untrained brain, so the difference is attributable to the training rather
# than to whatever else changed today.
#
#   scripts/overnight.sh            full run
#   RUNS=graded_core scripts/overnight.sh    a shorter one
set -uo pipefail
cd "$(dirname "$0")/.."

out="${OUT:-target/overnight/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$out"
brain="$out/brain.db"
base="$out/baseline.db"
spoon=./target/debug/spoon

cargo build -q --workspace || exit 1
echo "results: $out"

step() {
  local name="$1"; shift
  echo "== $name"
  # Never fail the run on one bad step: a corpus that trips on case 400 has
  # still taught the brain everything up to 399, and the log says what broke.
  SPOON_SCRATCH="$1" "$spoon" "${@:2}" > "$out/$name.log" 2>&1 \
    || echo "   (exited nonzero, see $out/$name.log)"
  grep -E "^(correct|ears|concepts):" "$out/$name.log" | sed 's/^/   /'
}

# What an untrained brain scores, for comparison.
step baseline-test "$base" bench graded_test --no-teaching

step teach       "$brain" teach data/curriculum/basics.json
step train       "$brain" bench graded_train
step train-facts "$brain" bench graded_facts
# The only number that means anything.
step trained-test "$brain" bench graded_test --no-teaching
step trained-facts "$brain" bench graded_facts --no-teaching

echo
echo "== summary"
for f in baseline-test trained-test trained-facts; do
  printf '%-16s %s\n' "$f" "$(grep -E '^correct:' "$out/$f.log" | head -1)"
done
