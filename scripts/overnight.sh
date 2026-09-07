#!/usr/bin/env bash
# Train a fresh brain, then score it on utterances it has never seen.
#
# The order is the whole point. Teaching is on for the curriculum and the
# training half, off for the test half, so the final number is what Spoon can
# do rather than what it has memorized. A baseline is measured first on an
# untrained brain, so the difference is attributable to the training rather
# than to whatever else changed today.
#
#   scripts/overnight.sh                          full run
#   TRAIN=smoke_train TEST=smoke_test scripts/overnight.sh    a smoke test
set -uo pipefail
cd "$(dirname "$0")/.."

out="${OUT:-target/overnight/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$out"
brain="$out/brain.db"
base="$out/baseline.db"
# A copy, taken once. Each step execs the binary fresh, so editing the source
# mid-run would silently score the trained half with different code than the
# baseline half and every comparison in the summary would be meaningless.
spoon="$out/spoon"
train_suite="${TRAIN:-graded_train}"
test_suite="${TEST:-graded_test}"
facts_train="${FACTS_TRAIN:-graded_facts_train}"
facts_test="${FACTS_TEST:-graded_facts_test}"

cargo build -q --workspace || exit 1
cp ./target/debug/spoon "$spoon"
git rev-parse HEAD > "$out/commit" 2>/dev/null || true
echo "results: $out  (binary pinned at $(cut -c1-8 "$out/commit" 2>/dev/null))"

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
step baseline-test "$base" bench "$test_suite" --no-teaching

step teach       "$brain" teach --file data/curriculum/basics.json ${LESSONS:+--limit "$LESSONS"}
step train       "$brain" bench "$train_suite"
step train-facts "$brain" bench "$facts_train"
# The only number that means anything.
step trained-test "$brain" bench "$test_suite" --no-teaching
step trained-facts "$brain" bench "$facts_test" --no-teaching

echo
echo "== summary"
for f in baseline-test trained-test trained-facts; do
  printf '%-16s %s\n' "$f" "$(grep -E '^correct:' "$out/$f.log" | head -1)"
done
