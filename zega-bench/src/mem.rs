// zega-mem: bytes per node and per relationship of the in-memory engine
// (zegadb/zega#100), plus the query timings that must not regress while the
// memory comes down.
//
//   zega-mem run <shape> <nodes> [--rels] [--queries] [--via snapshot|zql]
//   zega-mem table <results.jsonl>...
//
// One `run` is one process and prints one JSON line, so every number starts
// from a fresh allocator. `scripts/mem-bench.sh` runs the matrix.
//
// Shapes (each node has one type label unless noted, ~3 relationships per
// node with --rels):
//   fly5     the Fly benchmark behind the 5.5 KB figure: Item { n name city
//            score at } with three `next` links each
//   flights  the Flights sample's shape: Airport { code name city at }, one
//            BASE link to a Country and two ROUTE links; 1 Country per 100
//   cities   the Cities sample's shape: 10% City { name country at } with 20
//            ROUTE links each, 90% Place { name kind at } with one IN link
//   mixed    synthetic: two labels per node and 4-8 properties of mixed
//            types (Int, String, Float, Bool, Point), three LINK links
//
// Heap is exact: a counting global allocator wraps the system allocator, so
// "live bytes" is what the program holds, not what the OS has mapped. RSS is
// reported as well, as a secondary number.
//
// The graph is built the way a restart builds it, from a snapshot
// (`Zega::restore_bytes`), then one query with the schema declares its
// `unique` indexes. `--via zql` instead imports CSV through ZQL `mutation
// csv`, 10,000 rows per statement, the way the Fly benchmark loaded it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value as Json};
use zega::location::Point;
use zega::{Value, Zega};

// ---------------------------------------------------------------- allocator

static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            grew(layout.size() as i64);
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            grew(layout.size() as i64);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            let delta = new_size as i64 - layout.size() as i64;
            let now = LIVE.fetch_add(delta, Ordering::Relaxed) + delta;
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        new_ptr
    }
}

fn grew(size: i64) {
    let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(now, Ordering::Relaxed);
    ALLOCS.fetch_add(1, Ordering::Relaxed);
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn live() -> i64 {
    LIVE.load(Ordering::Relaxed)
}

fn reset_peak() {
    PEAK.store(live(), Ordering::Relaxed);
}

fn peak() -> i64 {
    PEAK.load(Ordering::Relaxed)
}

fn rss() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        * 1024
}

fn peak_rss() -> u64 {
    // SAFETY: getrusage writes into the zeroed struct we own.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64
    } else {
        usage.ru_maxrss as u64 * 1024
    }
}

// ---------------------------------------------------------------- data

/// splitmix64: the same graph for the same (shape, n), on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn text(v: impl Into<String>) -> Value {
    Value::from(v.into())
}

fn point(rng: &mut Rng) -> Value {
    let lat = rng.unit() * 170.0 - 85.0;
    let lon = rng.unit() * 358.0 - 179.0;
    Value::Point(Point::new(lat, lon).expect("in range"))
}

/// A node or relationship as the snapshot stores it (the field order of
/// zega's own `Node` and `Relationship`, which bincode depends on).
#[derive(Serialize)]
struct WireNode {
    id: u64,
    labels: Vec<String>,
    props: HashMap<String, Value>,
}

#[derive(Serialize)]
struct WireRel {
    id: u64,
    kind: String,
    from: u64,
    to: u64,
    props: HashMap<String, Value>,
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Fly5,
    Flights,
    Cities,
    Mixed,
}

impl Shape {
    fn parse(name: &str) -> Shape {
        match name {
            "fly5" => Shape::Fly5,
            "flights" => Shape::Flights,
            "cities" => Shape::Cities,
            "mixed" => Shape::Mixed,
            _ => usage(),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Shape::Fly5 => "fly5",
            Shape::Flights => "flights",
            Shape::Cities => "cities",
            Shape::Mixed => "mixed",
        }
    }

    fn schema(self) -> &'static str {
        match self {
            Shape::Fly5 => {
                "schema { type Item { n: Int name: String city: String score: Int \
                 at: Point from (lat, lon) next -> Item[] } } unique { Item { n } }"
            }
            Shape::Flights => {
                "schema { type Country { name: String iso: String airports: BASE <- Airport[] } \
                 type Airport { code: String name: String city: String at: Point from (lat, lon) \
                 country: BASE -> Country route: ROUTE -> Airport[] inbound: ROUTE <- Airport[] } } \
                 unique { Airport { code } Country { iso } }"
            }
            Shape::Cities => {
                "schema { type City { name: String country: String at: Point from (lat, lon) \
                 route: ROUTE -> City[] inbound: ROUTE <- City[] places: IN <- Place[] } \
                 type Place { name: String kind: String at: Point from (lat, lon) city: IN -> City } } \
                 unique { City { name } Place { name } }"
            }
            Shape::Mixed => {
                "schema { type Thing { id: Int name: String score: Float active: Bool \
                 at: Point from (lat, lon) tag: String count: Int note: String link: LINK -> Thing[] } } \
                 unique { Thing { id } }"
            }
        }
    }
}

const KINDS: [&str; 12] = [
    "park", "cafe", "museum", "station", "school", "library", "market", "church", "stadium",
    "hotel", "gallery", "bridge",
];

fn rel(edges: &mut Vec<WireRel>, kind: &str, from: u64, to: u64) {
    let id = edges.len() as u64 + 1;
    edges.push(WireRel {
        id,
        kind: kind.to_string(),
        from,
        to,
        props: HashMap::new(),
    });
}

/// Every node and relationship of `shape` at `n` nodes, in id order.
fn generate(shape: Shape, n: u64, rels: bool) -> (Vec<WireNode>, Vec<WireRel>) {
    let mut rng = Rng(0x2e6a_0100 ^ n);
    let mut nodes = Vec::with_capacity(n as usize);
    let mut edges = Vec::new();
    match shape {
        Shape::Fly5 => {
            for i in 0..n {
                let props = HashMap::from([
                    ("n".to_string(), Value::from(i as i64)),
                    ("name".to_string(), text(format!("item-{i}"))),
                    ("city".to_string(), text(format!("c{}", i % 500))),
                    ("score".to_string(), Value::from((i % 1000) as i64)),
                    ("at".to_string(), point(&mut rng)),
                ]);
                nodes.push(WireNode { id: i + 1, labels: vec!["Item".into()], props });
            }
            if rels {
                for i in 0..n {
                    for _ in 0..3 {
                        let to = rng.below(n) + 1;
                        rel(&mut edges, "next", i + 1, to);
                    }
                }
            }
        }
        Shape::Flights => {
            let countries = (n / 100).max(1);
            for c in 0..countries {
                let props = HashMap::from([
                    ("name".to_string(), text(format!("Country {c}"))),
                    ("iso".to_string(), text(format!("{}{}", (b'A' + (c / 26 % 26) as u8) as char, c))),
                ]);
                nodes.push(WireNode { id: c + 1, labels: vec!["Country".into()], props });
            }
            let airports = n - countries;
            for a in 0..airports {
                let props = HashMap::from([
                    ("code".to_string(), text(format!("A{a:06}"))),
                    ("name".to_string(), text(format!("Airport number {a} International"))),
                    ("city".to_string(), text(format!("City {}", a % 5000))),
                    ("at".to_string(), point(&mut rng)),
                ]);
                nodes.push(WireNode { id: countries + a + 1, labels: vec!["Airport".into()], props });
            }
            if rels {
                for a in 0..airports {
                    let id = countries + a + 1;
                    rel(&mut edges, "BASE", id, rng.below(countries) + 1);
                    for _ in 0..2 {
                        rel(&mut edges, "ROUTE", id, countries + rng.below(airports) + 1);
                    }
                }
            }
        }
        Shape::Cities => {
            let cities = (n / 10).max(1);
            for c in 0..cities {
                let props = HashMap::from([
                    ("name".to_string(), text(format!("City {c}"))),
                    ("country".to_string(), text(format!("C{}", c % 200))),
                    ("at".to_string(), point(&mut rng)),
                ]);
                nodes.push(WireNode { id: c + 1, labels: vec!["City".into()], props });
            }
            let places = n - cities;
            for p in 0..places {
                let props = HashMap::from([
                    ("name".to_string(), text(format!("Place {p}"))),
                    ("kind".to_string(), text(KINDS[(p % 12) as usize])),
                    ("at".to_string(), point(&mut rng)),
                ]);
                nodes.push(WireNode { id: cities + p + 1, labels: vec!["Place".into()], props });
            }
            if rels {
                for p in 0..places {
                    rel(&mut edges, "IN", cities + p + 1, rng.below(cities) + 1);
                }
                for c in 0..cities {
                    for _ in 0..20 {
                        rel(&mut edges, "ROUTE", c + 1, rng.below(cities) + 1);
                    }
                }
            }
        }
        Shape::Mixed => {
            const EXTRA: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
            for i in 0..n {
                let k = 4 + (rng.below(5) as usize);
                let mut props = HashMap::new();
                props.insert("id".to_string(), Value::from(i as i64));
                let name_len = 8 + rng.below(17) as usize;
                let mut name = format!("t{i}-");
                while name.len() < name_len {
                    name.push('x');
                }
                props.insert("name".to_string(), text(name));
                props.insert("score".to_string(), Value::from_f64(rng.unit() * 100.0));
                props.insert("active".to_string(), Value::from(rng.below(2) == 0));
                if k > 4 {
                    props.insert("at".to_string(), point(&mut rng));
                }
                if k > 5 {
                    props.insert("tag".to_string(), text(format!("tag{}", rng.below(50))));
                }
                if k > 6 {
                    props.insert("count".to_string(), Value::from(rng.below(10_000) as i64));
                }
                if k > 7 {
                    let len = 40 + rng.below(41) as usize;
                    props.insert("note".to_string(), text("n".repeat(len)));
                }
                let labels = vec!["Thing".to_string(), EXTRA[(i % 8) as usize].to_string()];
                nodes.push(WireNode { id: i + 1, labels, props });
            }
            if rels {
                for i in 0..n {
                    for _ in 0..3 {
                        rel(&mut edges, "LINK", i + 1, rng.below(n) + 1);
                    }
                }
            }
        }
    }
    (nodes, edges)
}

/// A snapshot in the format every zega since the WAL v2 reads: bincode of
/// `{ nodes: map id -> Node, relationships: map id -> Relationship }`.
fn snapshot(nodes: &[WireNode], rels: &[WireRel]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(nodes.len() as u64).to_le_bytes());
    for node in nodes {
        bincode::serialize_into(&mut bytes, &node.id).unwrap();
        bincode::serialize_into(&mut bytes, node).unwrap();
    }
    bytes.extend_from_slice(&(rels.len() as u64).to_le_bytes());
    for rel in rels {
        bincode::serialize_into(&mut bytes, &rel.id).unwrap();
        bincode::serialize_into(&mut bytes, rel).unwrap();
    }
    bytes
}

// ---------------------------------------------------------------- zql import

fn csv_field(value: &Value) -> String {
    let text = value.to_string();
    if text.contains([',', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// Load `nodes`/`rels` of the fly5 shape through ZQL `mutation csv`, in
/// statements of `chunk` rows, as the Fly benchmark did over HTTP.
fn import_fly5(zega: &Zega, nodes: &[WireNode], rels: &[WireRel], chunk: usize) {
    let schema = Shape::Fly5.schema();
    let load_nodes = format!(
        "{schema}\nmutation csv [\"nodes.csv\"] {{ Item(n: $n && name: $name && city: $city && score: $score) {{ n }} }}"
    );
    let load_links = format!(
        "{schema}\nmutation csv [\"links.csv\"] {{ Item(n: $from) {{ next -> link Item(n: $to) {{ n }} }} }}"
    );
    for part in nodes.chunks(chunk) {
        let mut csv = String::from("n,name,city,score,lat,lon\n");
        for node in part {
            let p = &node.props;
            let Value::Point(at) = &p["at"] else { unreachable!() };
            csv.push_str(&format!(
                "{},{},{},{},{},{}\n",
                csv_field(&p["n"]),
                csv_field(&p["name"]),
                csv_field(&p["city"]),
                csv_field(&p["score"]),
                at.lat(),
                at.lon()
            ));
        }
        let sources = HashMap::from([("nodes.csv".to_string(), csv)]);
        zega.apply_zql_with_sources(&load_nodes, &sources).expect("node import");
    }
    for part in rels.chunks(chunk) {
        let mut csv = String::from("from,to\n");
        for rel in part {
            csv.push_str(&format!("{},{}\n", rel.from - 1, rel.to - 1));
        }
        let sources = HashMap::from([("links.csv".to_string(), csv)]);
        zega.apply_zql_with_sources(&load_links, &sources).expect("link import");
    }
}

// ---------------------------------------------------------------- queries

struct Timing {
    name: &'static str,
    samples: Vec<f64>,
}

impl Timing {
    fn json(&self) -> Json {
        let mut v = self.samples.clone();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        json!({
            "query": self.name,
            "runs": v.len(),
            "p50_us": at(0.5),
            "p99_us": at(0.99),
            "mean_us": mean,
        })
    }
}

fn time(
    zega: &Zega,
    schema: &str,
    name: &'static str,
    runs: usize,
    mut query: impl FnMut(usize) -> String,
) -> Timing {
    // One untimed run warms caches and checks the query is valid here.
    zega.run_lang(schema, &query(runs)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut samples = Vec::with_capacity(runs);
    for i in 0..runs {
        let q = query(i);
        let t = Instant::now();
        let out = zega.run_lang(schema, &q).unwrap_or_else(|e| panic!("{name}: {e}"));
        samples.push(t.elapsed().as_secs_f64() * 1e6);
        std::hint::black_box(out);
    }
    Timing { name, samples }
}

fn queries(zega: &Zega, shape: Shape, n: u64) -> Vec<Timing> {
    let schema = shape.schema();
    let mut rng = Rng(42);
    let mut key = move || rng.below(n / 2);
    let scans = if n >= 1_000_000 { 5 } else { 20 };
    match shape {
        Shape::Fly5 => vec![
            time(zega, schema, "lookup", 2000, |_| format!("{{ Item(n: {}) {{ n name city score at }} }}", key())),
            time(zega, schema, "hop2", 2000, |_| {
                format!("{{ Item(n: {}) {{ n next -> Item {{ n next -> Item {{ n }} }} }} }}", key())
            }),
            time(zega, schema, "scan", scans, |i| {
                format!("{{ Item(city: \"c{}\" && score >= 900) {{ n }} }}", i % 500)
            }),
            time(zega, schema, "heavy", scans, |i| {
                format!(
                    "{{ Item(city: \"c{}\" && score >= 900) order by @distance(at, @point(51.05, -114.05)) limit 10 {{ n name score }} }}",
                    i % 500
                )
            }),
            time(zega, schema, "near", 200, |i| {
                format!(
                    "{{ Item(@distance(at, @point({}, {})) <= 50000) {{ n }} }}",
                    (i % 120) as i64 - 60,
                    (i % 300) as i64 - 150
                )
            }),
            time(zega, schema, "write", 500, |i| {
                format!(
                    "mutation {{ Item(n: {} && name: \"w{i}\" && city: \"c1\" && score: 1 && at: @point(51.0, -114.0)) {{ n }} }}",
                    n as usize + 10 + i
                )
            }),
        ],
        Shape::Flights => {
            let countries = (n / 100).max(1);
            let airports = n - countries;
            let mut r = Rng(7);
            let mut code = move || r.below(airports);
            vec![
                time(zega, schema, "lookup", 2000, |_| {
                    format!("{{ Airport(code: \"A{:06}\") {{ code name city at }} }}", code())
                }),
                time(zega, schema, "hop2", 2000, |_| {
                    format!(
                        "{{ Airport(code: \"A{:06}\") {{ code route -> Airport {{ code country -> Country {{ name }} }} }} }}",
                        code()
                    )
                }),
                time(zega, schema, "scan", scans, |i| {
                    format!("{{ Airport(city: \"City {}\") limit 10000000 {{ code }} }}", i * 37 % 5000)
                }),
                time(zega, schema, "near", 200, |i| {
                    format!(
                        "{{ Airport(@distance(at, @point({}, {})) <= 50000) {{ code }} }}",
                        (i % 120) as i64 - 60,
                        (i % 300) as i64 - 150
                    )
                }),
            ]
        }
        Shape::Cities => {
            let cities = (n / 10).max(1);
            let mut r = Rng(9);
            let mut city = move || r.below(cities);
            vec![
                time(zega, schema, "lookup", 2000, |_| format!("{{ City(name: \"City {}\") {{ name country at }} }}", city())),
                time(zega, schema, "hop2", 2000, |_| {
                    format!(
                        "{{ City(name: \"City {}\") {{ name route -> City {{ name places <- Place {{ name }} }} }} }}",
                        city()
                    )
                }),
                time(zega, schema, "scan", scans, |i| {
                    format!("{{ Place(kind: \"{}\") limit 10000000 {{ name }} }}", KINDS[i % 12])
                }),
            ]
        }
        Shape::Mixed => vec![
            time(zega, schema, "lookup", 2000, |_| format!("{{ Thing(id: {}) {{ id name score active }} }}", key())),
            time(zega, schema, "hop2", 2000, |_| {
                format!("{{ Thing(id: {}) {{ id link -> Thing {{ id link -> Thing {{ id }} }} }} }}", key())
            }),
            time(zega, schema, "scan", scans, |i| format!("{{ Thing(tag: \"tag{}\" && active: true) limit 10000000 {{ id }} }}", i % 50)),
        ],
    }
}

// ---------------------------------------------------------------- run

fn run(args: &[String]) {
    let shape = Shape::parse(args.first().map(String::as_str).unwrap_or_else(|| usage()));
    let n: u64 = args
        .get(1)
        .and_then(|s| s.replace('_', "").parse().ok())
        .unwrap_or_else(|| usage());
    let rels = args.iter().any(|a| a == "--rels");
    let with_queries = args.iter().any(|a| a == "--queries");
    let via_zql = args.windows(2).any(|w| w[0] == "--via" && w[1] == "zql");
    if via_zql && shape != Shape::Fly5 {
        eprintln!("--via zql is only wired for fly5");
        std::process::exit(2);
    }

    let zega = Zega::in_memory().build().expect("zega");
    let base = live();
    let base_rss = rss();

    let (nodes, edges) = generate(shape, n, rels);
    let node_count = nodes.len() as u64;
    let rel_count = edges.len() as u64;

    let started = Instant::now();
    let load_peak;
    if via_zql {
        let before = live();
        reset_peak();
        import_fly5(&zega, &nodes, &edges, 10_000);
        load_peak = peak() - before;
        drop(nodes);
        drop(edges);
    } else {
        let bytes = snapshot(&nodes, &edges);
        drop(nodes);
        drop(edges);
        let before = live();
        reset_peak();
        zega.restore_bytes(&bytes).expect("restore");
        load_peak = peak() - before;
        drop(bytes);
    }
    let load_ms = started.elapsed().as_secs_f64() * 1e3;
    let after_load = live();

    // The first statement with the schema declares its unique indexes.
    let started = Instant::now();
    let probe = match shape {
        Shape::Fly5 => "{ Item(n: 0) { n } }",
        Shape::Flights => "{ Airport(code: \"A000000\") { code } }",
        Shape::Cities => "{ City(name: \"City 0\") { name } }",
        Shape::Mixed => "{ Thing(id: 0) { id } }",
    };
    zega.run_lang(shape.schema(), probe).expect("probe");
    let index_ms = started.elapsed().as_secs_f64() * 1e3;
    let settled = live();
    let settled_rss = rss();

    let graph_bytes = settled - base;
    let mut out = json!({
        "shape": shape.name(),
        "via": if via_zql { "zql" } else { "snapshot" },
        "n": n,
        "nodes": node_count,
        "rels": rel_count,
        "heap_bytes": graph_bytes,
        "heap_before_indexes": after_load - base,
        "bytes_per_node": graph_bytes as f64 / node_count as f64,
        "load_peak_bytes": load_peak,
        "rss_bytes": settled_rss.saturating_sub(base_rss),
        "peak_rss_bytes": peak_rss(),
        "rss_per_node": settled_rss.saturating_sub(base_rss) as f64 / node_count as f64,
        "load_ms": load_ms,
        "index_ms": index_ms,
        "allocs": ALLOCS.load(Ordering::Relaxed),
    });
    if with_queries {
        let timings: Vec<Json> = queries(&zega, shape, node_count).iter().map(Timing::json).collect();
        out["queries"] = Json::Array(timings);
    }
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    writeln!(lock, "{out}").unwrap();
}

// ---------------------------------------------------------------- table

/// Markdown tables from one or more result files: memory per shape and size
/// (bytes per node from runs without relationships, bytes per relationship
/// from the difference), and query timings.
fn table(files: &[String]) {
    for file in files {
        let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{file}: {e}"));
        let rows: Vec<Json> = text
            .lines()
            .filter(|l| l.starts_with('{'))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        println!("### {file}\n");
        println!("| shape | via | nodes | rels | heap B/node (no rels) | heap B/rel | heap B/node (with rels) | RSS B/node (with rels) | load peak B/node | load ms |");
        println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|");
        let key = |r: &Json| (r["shape"].as_str().unwrap().to_string(), r["via"].as_str().unwrap().to_string(), r["n"].as_u64().unwrap());
        let mut done = Vec::new();
        for r in rows.iter().filter(|r| r["rels"].as_u64() > Some(0)) {
            let k = key(r);
            if done.contains(&k) {
                continue;
            }
            done.push(k.clone());
            let bare = rows.iter().find(|b| key(b) == k && b["rels"].as_u64() == Some(0));
            let nodes = r["nodes"].as_f64().unwrap();
            let rels = r["rels"].as_f64().unwrap();
            let with = r["heap_bytes"].as_f64().unwrap();
            let (per_node, per_rel) = match bare {
                Some(b) => {
                    let without = b["heap_bytes"].as_f64().unwrap();
                    (format!("{:.0}", without / nodes), format!("{:.0}", (with - without) / rels))
                }
                None => ("–".into(), "–".into()),
            };
            println!(
                "| {} | {} | {} | {} | {} | {} | {:.0} | {:.0} | {:.0} | {:.0} |",
                k.0,
                k.1,
                nodes,
                rels,
                per_node,
                per_rel,
                with / nodes,
                r["rss_per_node"].as_f64().unwrap(),
                r["load_peak_bytes"].as_f64().unwrap() / nodes,
                r["load_ms"].as_f64().unwrap(),
            );
        }
        println!();
        let timed: Vec<&Json> = rows.iter().filter(|r| r.get("queries").is_some()).collect();
        if !timed.is_empty() {
            println!("| shape | nodes | query | runs | p50 µs | p99 µs | mean µs |");
            println!("|---|---:|---|---:|---:|---:|---:|");
            for r in timed {
                for q in r["queries"].as_array().unwrap() {
                    println!(
                        "| {} | {} | {} | {} | {:.1} | {:.1} | {:.1} |",
                        r["shape"].as_str().unwrap(),
                        r["nodes"],
                        q["query"].as_str().unwrap(),
                        q["runs"],
                        q["p50_us"].as_f64().unwrap(),
                        q["p99_us"].as_f64().unwrap(),
                        q["mean_us"].as_f64().unwrap(),
                    );
                }
            }
            println!();
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: zega-mem run <fly5|flights|cities|mixed> <nodes> [--rels] [--queries] [--via snapshot|zql]\n       zega-mem table <results.jsonl>..."
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("table") => table(&args[1..]),
        _ => usage(),
    }
}
