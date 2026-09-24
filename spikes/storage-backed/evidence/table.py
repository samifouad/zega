"""Tabulate the native evidence: python3 table.py [dir]"""
import json, os, sys
d = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(__file__)
def load(name):
    p = os.path.join(d, name + ".json")
    try:
        return json.load(open(p))
    except Exception:
        return None
shapes = ["point", "point_hot", "filter", "two_hop", "two_hop_hot", "scan_limit", "scan_all", "create", "link"]
for t in ["20k", "200k", "2m"]:
    L = load(f"load-{t}"); S = load(f"sqlite-{t}-16mb"); S64 = load(f"sqlite-{t}-64mb"); SF = load(f"sqlite-{t}-16mb-syncfull")
    M = load(f"memory-{t}"); MW = load(f"memory-wal-{t}"); Z = load(f"seam-{t}")
    print(f"== {t}")
    if L: print(f"  sqlite db {L['db_mb']} MB ({L['bytes_per_node']} B/node), load {L['load_ms']/1000:.0f}s, tables {L['tables_mb']}")
    if S: print(f"  sqlite 16MB: rss {S['rss_mb']} peak {S['peak_rss_mb']} open {S['open_us']}us first query {S['first_query_us']}us")
    if S64: print(f"  sqlite 64MB: rss {S64['rss_mb']} peak {S64['peak_rss_mb']}")
    if M: print(f"  memory: rss {M['rss_mb']} graph {M['graph_mb']} MB peak {M['peak_rss_mb']} load {M['load_ms']}ms")
    print(f"  {'shape':12} {'mem p50/p99':>18} {'parse':>6} {'seam':>7} {'sql16 p50/p99':>18} {'sql64 p50/p99':>18} {'syncFULL':>16} {'memWAL':>16} {'stmts':>6} {'rows':>8} {'hit':>5}")
    for s in shapes:
        f = lambda x, k="p50_us": (f"{x['queries'][s][k]:.0f}" if x and s in x.get('queries', {}) else "-")
        pr = f"{M['parse_overhead'][s]['p50_us']:.0f}" if M and s in M.get('parse_overhead', {}) else "-"
        q = S['queries'].get(s, {}) if S else {}
        print(f"  {s:12} {f(M)+'/'+f(M,'p99_us'):>18} {pr:>6} {f(Z):>7} {f(S)+'/'+f(S,'p99_us'):>18} {f(S64)+'/'+f(S64,'p99_us'):>18} {f(SF)+'/'+f(SF,'p99_us'):>16} {f(MW)+'/'+f(MW,'p99_us'):>16} {q.get('statements_per_query','-'):>6} {q.get('rows_read_per_query','-'):>8} {str(q.get('cache_hit_rate','-')):>5}")
