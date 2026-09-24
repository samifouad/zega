"""Print the Cloudflare result tables (markdown) from a bench result file.

    python3 bench/tables.py results/run-1.json
"""
import json
import sys

SHAPES = [
    ("point", "point read (unique key)"),
    ("one_hop", "1-hop (3 nodes)"),
    ("two_hop", "2-hop (12 nodes)"),
    ("filter", "indexed filter (~20 rows)"),
    ("scan_limit", "scan + limit 10"),
    ("scan_all", "full scan, no match"),
]


def ms(s, side="client_ms"):
    if not s or s.get(side, {}).get("p50") is None:
        return "–"
    return f"{s[side]['p50']:g} / {s[side]['p95']:g}"


def main(path):
    r = json.load(open(path))
    t = r["targets"]
    do, d1 = t.get("durable_object", {}), t.get("d1", {})
    sizes = sorted({*do.keys(), *d1.keys()}, key=int)
    print("Client ms p50 / p95 (server-side in parentheses: Worker → object or D1)\n")
    head = "| | " + " | ".join(f"{int(n)//1000}k DO | {int(n)//1000}k D1 engine | {int(n)//1000}k D1 compiled" for n in sizes) + " |"
    print(head)
    print("|---" * (1 + 3 * len(sizes)) + "|")
    for key, label in SHAPES:
        cells = []
        for n in sizes:
            cells.append(f"{ms(do.get(n, {}).get('reads', {}).get(key))} ({ms(do.get(n, {}).get('reads', {}).get(key), 'server_ms')})")
            cells.append(f"{ms(d1.get(n, {}).get('reads_engine', {}).get(key))} ({ms(d1.get(n, {}).get('reads_engine', {}).get(key), 'server_ms')})")
            cells.append(f"{ms(d1.get(n, {}).get('reads_compiled', {}).get(key))} ({ms(d1.get(n, {}).get('reads_compiled', {}).get(key), 'server_ms')})")
        print(f"| {label} | " + " | ".join(cells) + " |")
    for key in ["create", "link"]:
        cells = []
        for n in sizes:
            cells.append(ms(do.get(n, {}).get("writes", {}).get(key)))
            cells.append(ms(d1.get(n, {}).get("writes", {}).get(key)))
            cells.append("")
        print(f"| {key} (write) | " + " | ".join(cells) + " |")
    cells = []
    for n in sizes:
        c = do.get(n, {}).get("cold", {}).get("point")
        cells += [f"{ms(c)} ({c.get('confirmed_cold') if c else '–'} confirmed)" if c else "–", "", ""]
    print("| cold point read (object evicted) | " + " | ".join(cells) + " |")
    cells = []
    for n in sizes:
        cells += [f"{(do.get(n, {}).get('storage', {}).get('database_bytes') or 0)/1e6:.1f} MB",
                  f"{(d1.get(n, {}).get('storage', {}).get('database_bytes') or 0)/1e6:.1f} MB", ""]
    print("| storage | " + " | ".join(cells) + " |")
    print("\nBilled rows read / written per request (DO cursors; D1 meta)\n")
    print("| | " + " | ".join(f"{int(n)//1000}k DO | {int(n)//1000}k D1 engine (round trips)" for n in sizes) + " |")
    print("|---" * (1 + 2 * len(sizes)) + "|")
    for key, label in SHAPES + [("create", "create"), ("link", "link")]:
        cells = []
        for n in sizes:
            src = do.get(n, {}).get("writes" if key in ("create", "link") else "reads", {}).get(key)
            u = src["per_request"] if src else None
            cells.append(f"{u['billed_rows_read']:g} / {u['billed_rows_written']:g}" if u else "–")
            src = d1.get(n, {}).get("writes" if key in ("create", "link") else "reads_engine", {}).get(key)
            u = src["per_request"] if src else None
            cells.append(f"{u['billed_rows_read']:g} / {u['billed_rows_written']:g} ({u['round_trips']:g})" if u else "–")
        print(f"| {label} | " + " | ".join(cells) + " |")
    peaks = [do[n]["reads"]["scan_all"]["heap_peak_bytes"] for n in do if "scan_all" in do[n].get("reads", {})]
    if peaks:
        print(f"\nPeak Rust heap in the object: {max(peaks)/1e6:.1f} MB")


if __name__ == "__main__":
    main(sys.argv[1])
