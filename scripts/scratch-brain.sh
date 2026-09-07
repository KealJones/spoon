#!/usr/bin/env bash
# Run spoon against a throwaway brain.
#
# Exists because the brain at ~/.spoon/spoon-v2.db belongs to whoever is using
# it, and a test that starts by deleting it destroys whatever they were in the
# middle of. Ask how I know.
#
#   scripts/scratch-brain.sh repl
#   scripts/scratch-brain.sh --permissions bypass eval 'sum<list<1, 2, 3>>'
set -euo pipefail
scratch="${SPOON_SCRATCH:-$(mktemp -d)/scratch.db}"
mkdir -p "$(dirname "$scratch")"
echo "scratch brain: $scratch" >&2
SPOON_SCRATCH="$scratch" exec ./target/debug/spoon "$@"
