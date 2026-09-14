#!/bin/sh
# Count instructions, rather than time them. See docs/core.md.
#
#   ./scripts/icount.sh --quick            # priority shapes, ~1 min
#   ./scripts/icount.sh --save             # full corpus, minutes
#   ./scripts/icount.sh --compare          # …on the other build
#
# The wall-clock suite (scripts/perf.sh) answers "is it fast on this machine",
# and needs an interleaved baseline because it drifts by a few percent between
# runs. This answers "did we add work", and does not drift: callgrind is a
# simulator, so the same build counts the same instructions today and next month.
#
# Neither replaces the other. A change can be free in instructions and still cost
# real time through cache or branch behaviour — but a change that moves Ir has
# provably added work, and one that does not has provably not.
set -e
cd "$(dirname "$0")/.."

BIN=./target/release/examples/perf
DEFAULT_BASELINE=target/cha-perf/icount.tsv

# The three shapes people actually type (see docs/core.md), plus one probe each
# for the structural engine and the fuzz path, which is where the cheap gate
# earns its keep.
QUICK=".....
..o..e.
........;gdangboot
;..oting
;obelisk
(;oif)(;bel)
cathode\`1"

save=""; compare=""; tier=""; quick=""; reps=1; cache=""; patterns=""
while [ $# -gt 0 ]; do
  case "$1" in
    --save)    if [ $# -gt 1 ] && [ "${2#-}" = "$2" ]; then save="$2"; shift; else save="$DEFAULT_BASELINE"; fi ;;
    --compare) if [ $# -gt 1 ] && [ "${2#-}" = "$2" ]; then compare="$2"; shift; else compare="$DEFAULT_BASELINE"; fi ;;
    --tier)    tier="$2"; shift ;;
    --pattern) patterns="$patterns$2
"; shift ;;
    --reps)    reps="$2"; shift ;;
    --quick)   quick=1 ;;
    --cache)   cache=1 ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    *) echo "icount: unknown argument \`$1\`" >&2; exit 1 ;;
  esac
  shift
done

command -v valgrind >/dev/null 2>&1 || {
  echo "icount: valgrind is not installed." >&2
  echo "  sudo apt install valgrind" >&2
  echo "  This machine has no PMU (WSL2 does not expose one), so callgrind's" >&2
  echo "  simulation is the only route to an instruction count here." >&2
  exit 1
}

cargo build --release -p cha-core --example perf >&2

# Which patterns, as a tab-separated tier/pattern list. Taken from the corpus
# itself so the two lanes can never drift apart on what they measure.
all=$("$BIN" --list-tsv)
if [ -n "$patterns" ]; then
  work=$(printf '%s' "$patterns" | while IFS= read -r p; do [ -n "$p" ] && printf 'adhoc\t%s\n' "$p"; done)
elif [ -n "$quick" ]; then
  work=$(printf '%s\n' "$QUICK" | while IFS= read -r p; do
    [ -n "$p" ] || continue
    line=$(printf '%s\n' "$all" | awk -F'\t' -v pat="$p" '$2 == pat {print; exit}')
    [ -n "$line" ] || { echo "icount: --quick names \`$p\`, which is not in the corpus." >&2; exit 1; }
    printf '%s\n' "$line"
  done)
elif [ -n "$tier" ]; then
  work=$(printf '%s\n' "$all" | awk -F'\t' -v t="$tier" '$1 == t')
  [ -n "$work" ] || { echo "icount: no tier named \`$tier\`" >&2; exit 1; }
else
  work="$all"
fi

vg_extra=""
[ -n "$cache" ] && vg_extra="--cache-sim=yes --branch-sim=yes"

n=$(printf '%s\n' "$work" | wc -l)
echo "cha-core instruction count   ($n patterns, reps=$reps, git $(git describe --always --dirty 2>/dev/null || echo '?'))"
if [ -n "$cache" ]; then
  printf '%-13s %-26s %14s %12s %12s %12s\n' "tier" "pattern" "Ir" "I1 miss" "D1 miss" "mispred"
else
  printf '%-13s %-26s %14s\n' "tier" "pattern" "Ir"
fi

out=$(mktemp)
trap 'rm -f "$out"' EXIT
printf '%s\n' "$work" | while IFS="$(printf '\t')" read -r t p; do
  [ -n "$p" ] || continue
  # --collect-atstart=no plus a toggle on the named symbol counts the scan and
  # nothing else. Bracketing is not optional: the whole process is ~10x the scan,
  # and the arithmetic alternative (run N and 2N, subtract) is unsound because the
  # dedup HashSet is randomly seeded, so setup does not cancel between processes.
  log=$(valgrind --tool=callgrind --collect-atstart=no \
                 --toggle-collect=cha_icount_scan \
                 --callgrind-out-file=/dev/null $vg_extra \
                 "$BIN" --icount "$reps" --pattern "$p" 2>&1)
  ir=$(printf '%s\n' "$log" | grep -oE 'Collected : [0-9]+' | grep -oE '[0-9]+')
  [ -n "$ir" ] || { echo "icount: callgrind produced no count for \`$p\`" >&2; printf '%s\n' "$log" | tail -5 >&2; exit 1; }
  if [ -n "$cache" ]; then
    num() { printf '%s\n' "$log" | grep -E "$1" | head -1 | grep -oE '[0-9,]+$' | tr -d ','; }
    i1=$(num 'I1  misses'); d1=$(num 'D1  misses'); mp=$(num 'Mispredicts')
    printf '%-13s %-26s %14s %12s %12s %12s\n' "$t" "$p" "$ir" "${i1:-0}" "${d1:-0}" "${mp:-0}"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$t" "$p" "$ir" "${i1:-0}" "${d1:-0}" "${mp:-0}" >> "$out"
  else
    printf '%-13s %-26s %14s\n' "$t" "$p" "$ir"
    printf '%s\t%s\t%s\n' "$t" "$p" "$ir" >> "$out"
  fi
done

if [ -n "$compare" ]; then
  [ -f "$compare" ] || { echo "icount: no baseline at $compare" >&2; exit 1; }
  echo
  echo "--- compared with $compare ---"
  # No noise floor and no re-measurement: there is nothing to average. Any delta
  # is real work, which is the whole reason this lane exists.
  awk -F'\t' -v base="$compare" '
    BEGIN { while ((getline line < base) > 0) { split(line, f, "\t"); was[f[2]] = f[3] } 
            printf "%-26s %14s %14s %9s\n", "pattern", "was", "now", "delta" }
    { if ($2 in was) {
        d = was[$2] > 0 ? ($3 - was[$2]) / was[$2] * 100 : 0
        printf "%-26s %14s %14s %8.2f%%%s\n", $2, was[$2], $3, d, (d > 0.01 ? "  MORE" : (d < -0.01 ? "  LESS" : ""))
        if (d > 0.01) worse++; if (d < -0.01) better++
      } else printf "%-26s %14s %14s %9s\n", $2, "-", $3, "new"
    }
    END { printf "\n%d pattern(s) execute more instructions, %d fewer.\n", worse+0, better+0 }
  ' "$out"
fi

if [ -n "$save" ]; then
  mkdir -p "$(dirname "$save")"
  cp "$out" "$save"
  echo
  echo "baseline saved to $save"
fi
