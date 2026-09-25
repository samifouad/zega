#!/usr/bin/env bash
# Memory per node and per relationship, with query timings (zegadb/zega#100).
#
#   scripts/mem-bench.sh <out.jsonl> [zega-mem binary]
#
# Runs every shape at 100k and 1M nodes, once without relationships (bytes
# per node) and once with ~3 per node (bytes per relationship, total bytes
# per node, query timings), plus the Fly benchmark's ZQL CSV import at 100k.
# One process per line of output. Summarise with `zega-mem table <out.jsonl>`.
# QUERIES=0 skips the query timings (memory only).
set -euo pipefail
out="$1"
bin="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "$bin" ]; then
  cargo build --locked --release -p zega-bench --bin zega-mem --manifest-path "$root/Cargo.toml"
  bin="${CARGO_TARGET_DIR:-$root/target}/release/zega-mem"
fi
: > "$out"
queries=--queries
[ "${QUERIES:-1}" = 0 ] && queries=
for n in 100000 1000000; do
  for shape in fly5 flights cities mixed; do
    "$bin" run "$shape" "$n" >> "$out"
    "$bin" run "$shape" "$n" --rels $queries >> "$out"
  done
done
"$bin" run fly5 100000 --via zql >> "$out"
"$bin" run fly5 100000 --rels --via zql >> "$out"
echo "wrote $out" >&2
