#!/usr/bin/env bash
# Memory per node and per relationship, with query timings (zegadb/zega#100).
#
#   scripts/mem-bench.sh <out.jsonl> [zega-mem binary]
#
# Runs every shape at 100k and 1M nodes, once without relationships (bytes
# per node) and once with ~3 per node (bytes per relationship, total bytes
# per node, query timings), the Fly benchmark's ZQL CSV import at 100k, and
# a restart (a snapshot opened by a fresh process) for RSS.
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
# A restart: the snapshot opened by a fresh process, so its RSS is the graph's.
for n in 100000 1000000; do
  for shape in fly5 mixed; do
    dir="${TMPDIR:-$root/.tmp}/zega-mem-restart"
    rm -rf "$dir"
    "$bin" snapshot "$shape" "$n" --rels "$dir"
    "$bin" reopen "$shape" "$dir" >> "$out"
    rm -rf "$dir"
  done
done
# Churn (review M1): one full turnover of delete-and-create through ZQL.
"$bin" run fly5 100000 --rels --churn window >> "$out"
"$bin" run fly5 100000 --rels --churn random >> "$out"
if [ "${CHURN_1M:-1}" = 1 ]; then
  "$bin" run fly5 1000000 --rels --churn window >> "$out"
fi
echo "wrote $out" >&2
