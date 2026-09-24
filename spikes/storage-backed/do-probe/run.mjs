// SPIKE driver: seed N nodes, then time K reads per kind against K=0.
const base = process.argv[2] ?? "http://127.0.0.1:8799";
const n = Number(process.argv[3] ?? 200000);
const get = async (path) => {
  const t = performance.now();
  const r = await fetch(base + path);
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  const body = await r.json();
  return { ms: performance.now() - t, body };
};
for (let lo = 1; lo <= n; lo += 5000) await get(`/seed?lo=${lo}&hi=${Math.min(lo + 4999, n)}&n=${n}`);
const out = { n };
for (const kind of ["point", "adj", "scan"]) {
  const k = kind === "scan" ? 2000 : 50000;
  const base0 = [];
  const full = [];
  for (let r = 0; r < 5; r++) {
    base0.push((await get(`/probe?kind=${kind}&k=0&n=${n}`)).ms);
    const run = await get(`/probe?kind=${kind}&k=${k}&n=${n}`);
    full.push(run.ms);
    out[kind + "_rows_read_per_call"] = run.body.rowsRead / k;
  }
  base0.sort((a, b) => a - b);
  full.sort((a, b) => a - b);
  out[kind + "_us_per_call"] = Math.round(((full[2] - base0[2]) / k) * 1000 * 100) / 100;
}
console.log(JSON.stringify(out));
