"""Cost per free graph per month, from a bench result file.

    python3 bench/cost.py results/run-1.json

Prices: Cloudflare pages checked 24 Sep 2026 (USD), Workers Paid.
Units per request are the measured `per_request` averages; duration uses the
measured server-side mean (the object is only billed while it works: an
HTTP-only Durable Object is hibernation-eligible between requests).
"""
import json
import sys

WORKER_REQ = 0.30 / 1e6
DO_REQ = 0.15 / 1e6
DO_GBS = 12.50 / 1e6
DO_GB = 0.128
ROW_READ = 0.001 / 1e6
ROW_WRITE = 1.00 / 1e6
DO_STORE_GB_MONTH = 0.20
D1_STORE_GB_MONTH = 0.75

# The free-tier profile we price: a month of use at the proposed limits.
PROFILE = {
    "queries": 100_000,
    "writes": 10_000,
    "mix": {"point": 0.4, "one_hop": 0.2, "two_hop": 0.2, "filter": 0.1, "scan_limit": 0.1},
}


def per_query(shape, target, d1=False):
    u = shape["per_request"]
    cost = WORKER_REQ + u["billed_rows_read"] * ROW_READ + u["billed_rows_written"] * ROW_WRITE
    if not d1:
        cost += DO_REQ + DO_GB * (shape["server_ms"]["mean"] or 0) / 1000 * DO_GBS
    return cost


def month(size, d1=False, reads_key="reads"):
    reads = size[reads_key]
    q = sum(w * per_query(reads[s], size, d1) for s, w in PROFILE["mix"].items()) * PROFILE["queries"]
    w = (per_query(size["writes"]["create"], size, d1) + per_query(size["writes"]["link"], size, d1)) / 2 * PROFILE["writes"]
    gb = (size["storage"].get("database_bytes") or 0) / 1e9
    store = gb * (D1_STORE_GB_MONTH if d1 else DO_STORE_GB_MONTH)
    return {"queries_usd": round(q, 4), "writes_usd": round(w, 4), "storage_usd": round(store, 5), "total_usd": round(q + w + store, 4), "idle_usd": round(store, 5)}


def main(path):
    r = json.load(open(path))
    out = {}
    for n, size in r["targets"].get("durable_object", {}).items():
        out[f"do {n}"] = month(size)
    for n, size in r["targets"].get("d1", {}).items():
        out[f"d1 engine {n}"] = month(size, d1=True, reads_key="reads_engine")
        out[f"d1 compiled {n}"] = month(size, d1=True, reads_key="reads_compiled")
    print(json.dumps({"profile": PROFILE, "per_month": out}, indent=2))


if __name__ == "__main__":
    main(sys.argv[1])
