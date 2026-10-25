#!/usr/bin/env bash
# Run Aether's Kani (bounded model-checking) harnesses one at a time and print
# one `[proof]` line per harness. Usage:
#   scripts/run_kani.sh            # every harness
#   scripts/run_kani.sh NAME...    # only the named harnesses
# Env: KANI_TIMEOUT (seconds per harness, default 3600), KANI_LOG_DIR.
set -u
cd "$(dirname "$0")/.."
MANIFEST=proofs/Cargo.toml
TIMEOUT="${KANI_TIMEOUT:-3600}"
LOG_DIR="${KANI_LOG_DIR:-target/kani-logs}"
mkdir -p "$LOG_DIR"
if ! command -v cargo-kani >/dev/null 2>&1; then
  echo "[proof] kani=missing (install: cargo install --locked kani-verifier && cargo kani setup)"
  exit 2
fi
if [ "$#" -gt 0 ]; then
  harnesses=("$@")
else
  harnesses=()
  for f in proofs/src/*_proofs.rs; do
    mod=$(basename "$f" .rs)
    while read -r fn; do harnesses+=("$fn"); done < <(grep -A4 '#\[kani::proof\]' "$f" \
      | sed -n 's/^fn \([a-z0-9_]*\)().*/\1/p')
  done
fi
pass=0; fail=0
for h in "${harnesses[@]}"; do
  f=$(grep -l "^fn $h()" proofs/src/*_proofs.rs | head -n 1)
  if [ -z "$f" ]; then echo "[proof] harness=$h result=unknown"; fail=$((fail + 1)); continue; fi
  full="$(basename "$f" .rs)::$h"
  log="$LOG_DIR/$h.txt"
  start=$(date +%s)
  timeout "$TIMEOUT" cargo kani --manifest-path "$MANIFEST" --harness "$full" --exact >"$log" 2>&1
  rc=$?
  secs=$(( $(date +%s) - start ))
  checks=$(sed -n 's/^ \*\* \([0-9]*\) of \([0-9]*\) failed.*/\2/p' "$log" | tail -n 1)
  if [ "$rc" -eq 0 ] && grep -q '^VERIFICATION:- SUCCESSFUL' "$log"; then
    echo "[proof] harness=$h result=verified checks=${checks:-?} time=${secs}s"
    pass=$((pass + 1))
  else
    why=failed
    [ "$rc" -eq 124 ] && why=timeout
    grep -q 'run out of memory' "$log" && why=oom
    echo "[proof] harness=$h result=$why time=${secs}s log=$log"
    fail=$((fail + 1))
  fi
done
echo "[proof] summary harnesses=$((pass + fail)) verified=$pass failed=$fail kind=bounded-model-checked"
[ "$fail" -eq 0 ]
