//! `field *path -> Target`: fewest edges, Dijkstra `by &weight`, and A*
//! `toward at in unit`, checked against a brute-force search over every simple
//! route, plus the route's shape, bounds, ties and errors.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use zega::Zega;

const SCHEMA: &str = "schema {
  type Junction { n: Int at: Point road -> Junction[] { m?: Int km?: Float } }
}
unique { Junction { n } }";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Great-circle metres, written independently of the engine (atan2 form).
fn metres(a: (f64, f64), b: (f64, f64)) -> f64 {
    let lat = ((b.0 - a.0).to_radians() / 2.0).sin().powi(2);
    let lon = ((b.1 - a.1).to_radians() / 2.0).sin().powi(2);
    let h = (lat + a.0.to_radians().cos() * b.0.to_radians().cos() * lon).clamp(0.0, 1.0);
    2.0 * 6_371_008.8 * h.sqrt().atan2((1.0 - h).sqrt())
}

/// An edge `from -> to` with its weight in metres (None: no weight stored).
#[derive(Clone, Copy)]
struct Road {
    from: usize,
    to: usize,
    m: Option<i64>,
}

/// Junctions `n = 0..points.len()` are inserted in order, so node ids follow `n`.
fn build(points: &[(f64, f64)], roads: &[Road]) -> Zega {
    build_with(points, roads, Zega::in_memory().build().unwrap())
}

fn build_with(points: &[(f64, f64)], roads: &[Road], db: Zega) -> Zega {
    let nodes: Vec<Json> = points
        .iter()
        .enumerate()
        .map(|(n, (lat, lon))| json!({ "n": n, "at": { "lat": lat, "lon": lon } }))
        .collect();
    let sources = HashMap::from([
        ("nodes.json".to_string(), json!(nodes).to_string()),
        (
            "roads.json".to_string(),
            json!(roads
                .iter()
                .map(|road| match road.m {
                    Some(m) => json!({ "from": road.from, "to": road.to, "m": m, "km": m as f64 / 1000.0 }),
                    None => json!({ "from": road.from, "to": road.to }),
                })
                .collect::<Vec<_>>())
            .to_string(),
        ),
    ]);
    db.run_lang_with_sources(SCHEMA, "mutation json [\"nodes.json\"] { Junction(n: $n) { n } }", &sources)
        .unwrap();
    if !roads.is_empty() {
        db.run_lang_with_sources(
            SCHEMA,
            "mutation json [\"roads.json\"] { Junction(n: $from) { road -> link Junction(n: $to) { &m: $m &km: $km } } }",
            &sources,
        )
        .unwrap();
    }
    db
}

fn try_route(db: &Zega, start: usize, target: &str, how: &str) -> Result<Json, String> {
    let query = format!("{{ Junction(n: {start}) {{ road *path{how} -> Junction({target}) {{ n &m }} }} }}");
    db.run_lang(SCHEMA, &query)
        .map(|row| row["road"].clone())
        .map_err(|error| error.to_string())
}

fn route(db: &Zega, start: usize, target: &str, how: &str) -> Json {
    try_route(db, start, target, how).unwrap_or_else(|error| panic!("{how}: {error}"))
}

fn ns(route: &Json) -> Vec<usize> {
    route["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["n"].as_u64().unwrap() as usize)
        .collect()
}

/// The fewest edges and the least metres over every simple route from
/// `start` to any goal, found by trying them all.
fn brute(n: usize, roads: &[Road], start: usize, goal: &dyn Fn(usize) -> bool) -> Option<(usize, i64)> {
    fn go(
        at: usize,
        roads: &[Road],
        goal: &dyn Fn(usize) -> bool,
        seen: &mut Vec<bool>,
        hops: usize,
        cost: i64,
        best: &mut Option<(usize, i64)>,
    ) {
        if goal(at) {
            let (h, c) = best.get_or_insert((hops, cost));
            *h = (*h).min(hops);
            *c = (*c).min(cost);
            return; // Going on through a goal only makes the route longer.
        }
        for road in roads.iter().filter(|road| road.from == at) {
            if !seen[road.to] {
                seen[road.to] = true;
                go(road.to, roads, goal, seen, hops + 1, cost + road.m.unwrap(), best);
                seen[road.to] = false;
            }
        }
    }
    let mut seen = vec![false; n];
    seen[start] = true;
    let mut best = None;
    go(start, roads, goal, &mut seen, 0, 0, &mut best);
    best
}

/// Checks a returned route edge by edge against the roads it claims to use.
fn check_shape(route: &Json, roads: &[Road], start: usize, goal: &dyn Fn(usize) -> bool, weighted: bool) {
    let nodes = ns(route);
    let edges = route["edges"].as_array().unwrap();
    assert_eq!(nodes.len(), edges.len() + 1, "{route}");
    assert_eq!(nodes[0], start);
    assert!(goal(*nodes.last().unwrap()), "{route}");
    assert_eq!(route["hops"], json!(edges.len()));
    let ids: Vec<u64> = route["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["n"].as_u64().unwrap() + 1)
        .collect();
    let mut sum = 0;
    for (i, edge) in edges.iter().enumerate() {
        // Consecutive edges connect: edge i leaves node i and enters node i+1.
        assert_eq!(edge["from"].as_u64().unwrap(), ids[i], "{route}");
        assert_eq!(edge["to"].as_u64().unwrap(), ids[i + 1], "{route}");
        assert_eq!(edge["type"], "road");
        let m = edge["props"]["m"].as_i64().unwrap();
        assert!(roads.iter().any(|r| r.from == nodes[i] && r.to == nodes[i + 1] && r.m == Some(m)));
        // Each node after the start reads the edge that arrived at it.
        assert_eq!(route["nodes"][i + 1]["m"], json!(m));
        sum += m;
    }
    assert_eq!(route["nodes"][0]["m"], Json::Null);
    if weighted {
        assert_eq!(route["cost"], json!(sum), "{route}");
    } else {
        assert_eq!(route["cost"], json!(edges.len()));
    }
}

fn random_net(rng: &mut Rng) -> (Vec<(f64, f64)>, Vec<Road>) {
    let n = 3 + rng.below(8) as usize;
    let points: Vec<(f64, f64)> = (0..n)
        .map(|_| (51.0 + rng.unit() * 0.05, -114.1 + rng.unit() * 0.08))
        .collect();
    let mut roads = Vec::new();
    for _ in 0..rng.below(3 * n as u64) {
        let (from, to) = (rng.below(n as u64) as usize, rng.below(n as u64) as usize);
        if from == to {
            continue;
        }
        // At least the straight line, so A* may use it; sometimes much more.
        let line = metres(points[from], points[to]).ceil() as i64;
        let detour = match rng.below(4) {
            0 => 0,
            1 => rng.below(50) as i64,
            _ => rng.below(3000) as i64,
        };
        roads.push(Road { from, to, m: Some(line + detour) });
    }
    (points, roads)
}

#[test]
fn all_three_agree_with_brute_force_on_random_graphs() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let (mut reachable, mut unreachable, mut expanded_astar, mut expanded_dijkstra) = (0, 0, 0, 0);
    for round in 0..150 {
        let (points, roads) = random_net(&mut rng);
        let db = build(&points, &roads);
        let n = points.len();
        let start = rng.below(n as u64) as usize;
        // One goal, or every junction above a threshold (the cheapest wins).
        let pick = rng.below(n as u64) as usize;
        let above = round % 3 == 0;
        let target = if above { format!("n > {pick}") } else { format!("n: {pick}") };
        let goal = move |node: usize| if above { node > pick } else { node == pick };
        let expected = brute(n, &roads, start, &goal);
        let fewest = route(&db, start, &target, "");
        let before = db.nodes_expanded().unwrap();
        let dijkstra = route(&db, start, &target, " by &m");
        let middle = db.nodes_expanded().unwrap();
        let astar = route(&db, start, &target, " by &m toward at in m");
        let after = db.nodes_expanded().unwrap();
        let astar_km = route(&db, start, &target, " by &km toward at in km");
        match expected {
            None => {
                unreachable += 1;
                for (name, got) in [("fewest", &fewest), ("dijkstra", &dijkstra), ("astar", &astar), ("astar km", &astar_km)] {
                    assert_eq!(*got, Json::Null, "round {round} {name}: {got}");
                }
            }
            Some((hops, cost)) => {
                reachable += 1;
                check_shape(&fewest, &roads, start, &goal, false);
                check_shape(&dijkstra, &roads, start, &goal, true);
                check_shape(&astar, &roads, start, &goal, true);
                assert_eq!(fewest["hops"], json!(hops), "round {round}: {fewest}");
                assert_eq!(dijkstra["cost"], json!(cost), "round {round}: {dijkstra}");
                assert_eq!(astar["cost"], json!(cost), "round {round}: {astar}");
                // km is a Float field: the same route, a thousandth of the cost.
                let km = astar_km["cost"].as_f64().unwrap();
                assert!((km - cost as f64 / 1000.0).abs() < 1e-9, "round {round}: {astar_km}");
                // Bounds: exactly at the cost is in, just under it is out.
                assert_eq!(route(&db, start, &target, &format!("(cost <= {cost}) by &m"))["cost"], json!(cost));
                assert_eq!(route(&db, start, &target, &format!("(cost < {cost}) by &m toward at in m")), Json::Null);
                assert_eq!(route(&db, start, &target, &format!("(hops <= {hops})"))["hops"], json!(hops));
                if hops > 0 {
                    assert_eq!(route(&db, start, &target, &format!("(hops < {hops})")), Json::Null);
                }
            }
        }
        expanded_dijkstra += middle - before;
        expanded_astar += after - middle;
        assert!(after - middle <= middle - before, "round {round}: A* expanded more than Dijkstra");
    }
    println!(
        "150 random graphs: {reachable} reachable, {unreachable} unreachable; nodes expanded: Dijkstra {expanded_dijkstra}, A* {expanded_astar}"
    );
    assert!(reachable > 60 && unreachable > 10, "{reachable} {unreachable}");
}

/// A `side` x `side` grid of junctions about 100 m apart, with two-way roads
/// whose length is the straight line rounded up to the metre.
fn grid(side: usize) -> (Vec<(f64, f64)>, Vec<Road>) {
    let points: Vec<(f64, f64)> = (0..side * side)
        .map(|i| (51.0 + (i / side) as f64 * 0.0009, -114.0 + (i % side) as f64 * 0.0014))
        .collect();
    let mut roads = Vec::new();
    for i in 0..side * side {
        let (row, col) = (i / side, i % side);
        let mut link = |j: usize| {
            let m = metres(points[i], points[j]).ceil() as i64;
            roads.push(Road { from: i, to: j, m: Some(m) });
            roads.push(Road { from: j, to: i, m: Some(m) });
        };
        if col + 1 < side {
            link(i + 1);
        }
        if row + 1 < side {
            link(i + side);
        }
    }
    (points, roads)
}

#[test]
fn astar_expands_fewer_nodes_than_dijkstra_on_a_grid() {
    let side = 30;
    let (points, roads) = grid(side);
    let db = build(&points, &roads);
    let start = 15 * side; // the middle of the west edge
    let goal = 15 * side + 22;
    let expanded = |how: &str| {
        let before = db.nodes_expanded().unwrap();
        let found = route(&db, start, &format!("n: {goal}"), how);
        (db.nodes_expanded().unwrap() - before, found)
    };
    let (dijkstra_expanded, dijkstra) = expanded(" by &m");
    let (astar_expanded, astar) = expanded(" by &m toward at in m");
    let (fewest_expanded, fewest) = expanded("");
    println!("grid {side}x{side}: expanded Dijkstra {dijkstra_expanded}, A* {astar_expanded}, fewest edges {fewest_expanded}");
    assert_eq!(dijkstra["cost"], astar["cost"]);
    assert_eq!(fewest["hops"], json!(22));
    assert_eq!(ns(&astar), (start..=goal).collect::<Vec<_>>());
    assert!(dijkstra_expanded > 400, "{dijkstra_expanded}");
    assert!(astar_expanded * 4 < dijkstra_expanded, "A* {astar_expanded}, Dijkstra {dijkstra_expanded}");
    assert_eq!(astar_expanded, 22, "A* walks straight east");
}

/// 0 -> 1 -> 3 and 0 -> 2 -> 3 cost the same; so do the two ways round the
/// square 3 -> 4 -> 6 and 3 -> 5 -> 6.
fn ties() -> (Vec<(f64, f64)>, Vec<Road>) {
    let points = vec![(51.0, -114.0), (51.001, -114.0), (51.0, -113.998), (51.001, -113.998), (51.002, -113.998), (51.001, -113.996), (51.002, -113.996)];
    let road = |from, to| Road { from, to, m: Some(1000) };
    let roads = vec![road(0, 1), road(0, 2), road(1, 3), road(2, 3), road(3, 4), road(3, 5), road(4, 6), road(5, 6)];
    (points, roads)
}

#[test]
fn ties_break_by_node_id_whatever_order_the_roads_were_stored() {
    let (points, roads) = ties();
    let forward = build(&points, &roads);
    let mut reversed_roads = roads.clone();
    reversed_roads.reverse();
    let reversed = build(&points, &reversed_roads);
    for how in ["", " by &m", " by &m toward at in m"] {
        let a = route(&forward, 0, "n: 6", how);
        let b = route(&reversed, 0, "n: 6", how);
        assert_eq!(ns(&a), ns(&b), "{how}");
        assert_eq!(a, route(&forward, 0, "n: 6", how), "{how}: the same query twice");
        assert_eq!(a["cost"], if how.is_empty() { json!(4) } else { json!(4000) });
    }
    // Without a guess, the lower id wins every tie.
    assert_eq!(ns(&route(&forward, 0, "n: 6", "")), vec![0, 1, 3, 4, 6]);
    assert_eq!(ns(&route(&forward, 0, "n: 6", " by &m")), vec![0, 1, 3, 4, 6]);
    // Two goals at the same cost: the lower id.
    assert_eq!(ns(&route(&forward, 0, "n: 1 || n: 2", " by &m")), vec![0, 1]);
}

const ROADS: &str = "schema {
  type Junction { name: String at: Point road -> Junction[] { km: Float } }
}
unique { Junction { name } }
mutation { Junction(name: \"A\" && at: point(51.0, -114.0)) { name } }
mutation { Junction(name: \"B\" && at: point(51.0, -113.99)) { name } }
mutation { Junction(name: \"C\" && at: point(51.0, -113.98)) { name } }
mutation { Junction(name: \"D\" && at: point(51.01, -113.99)) { name } }
mutation { Junction(name: \"A\") { road -> link Junction(name: \"B\") { &km: 0.8 } } }
mutation { Junction(name: \"B\") { road -> link Junction(name: \"C\") { &km: 0.8 } } }
mutation { Junction(name: \"A\") { road -> link Junction(name: \"D\") { &km: 1.4 } } }
mutation { Junction(name: \"D\") { road -> link Junction(name: \"C\") { &km: 1.4 } } }
mutation { Junction(name: \"A\") { road -> link Junction(name: \"C\") { &km: 2.5 } } }
";

#[test]
fn a_route_returns_its_nodes_edges_and_cost() {
    let run = |query: &str| {
        let db = Zega::in_memory().build().unwrap();
        db.apply_zql(&format!("{ROADS}query {{ {query} }}")).unwrap().to_string()
    };
    // Fewest edges: the direct road.
    assert_eq!(
        run("Junction(name: \"A\") { name road *path -> Junction(name: \"C\") { name &km &hops } }"),
        r#"{"name":"A","road":{"cost":1,"edges":[{"from":1,"id":5,"props":{"km":2.5},"to":3,"type":"road"}],"hops":1,"nodes":[{"hops":0,"km":null,"name":"A"},{"hops":1,"km":2.5,"name":"C"}]}}"#
    );
    // Least km: through B.
    let expected = r#"{"name":"A","road":{"cost":1.6,"edges":[{"from":1,"id":1,"props":{"km":0.8},"to":2,"type":"road"},{"from":2,"id":2,"props":{"km":0.8},"to":3,"type":"road"}],"hops":2,"nodes":[{"hops":0,"km":null,"name":"A"},{"hops":1,"km":0.8,"name":"B"},{"hops":2,"km":0.8,"name":"C"}]}}"#;
    assert_eq!(run("Junction(name: \"A\") { name road *path by &km -> Junction(name: \"C\") { name &km &hops } }"), expected);
    assert_eq!(
        run("Junction(name: \"A\") { name road *path by &km toward at in km -> Junction(name: \"C\") { name &km &hops } }"),
        expected
    );
    // Unreachable, and outside the bound, are null rather than errors.
    assert_eq!(run("Junction(name: \"C\") { road *path by &km -> Junction(name: \"A\") { name } }"), r#"{"road":null}"#);
    assert_eq!(run("Junction(name: \"A\") { road *path(cost < 1.6) by &km -> Junction(name: \"C\") { name } }"), r#"{"road":null}"#);
    // Every start gets its own route.
    assert_eq!(
        run("Junction { name road *path by &km -> Junction(name: \"C\") { name } }"),
        r#"[{"name":"A","road":{"cost":1.6,"edges":[{"from":1,"id":1,"props":{"km":0.8},"to":2,"type":"road"},{"from":2,"id":2,"props":{"km":0.8},"to":3,"type":"road"}],"hops":2,"nodes":[{"name":"A"},{"name":"B"},{"name":"C"}]}},{"name":"B","road":{"cost":0.8,"edges":[{"from":2,"id":2,"props":{"km":0.8},"to":3,"type":"road"}],"hops":1,"nodes":[{"name":"B"},{"name":"C"}]}},{"name":"C","road":{"cost":0.0,"edges":[],"hops":0,"nodes":[{"name":"C"}]}},{"name":"D","road":{"cost":1.4,"edges":[{"from":4,"id":4,"props":{"km":1.4},"to":3,"type":"road"}],"hops":1,"nodes":[{"name":"D"},{"name":"C"}]}}]"#
    );
}

#[test]
fn bad_weights_are_errors_that_name_the_edge() {
    let points = [(51.0, -114.0), (51.0, -113.99), (51.0, -113.98)];
    let line = metres(points[0], points[1]).ceil() as i64;
    // A missing weight.
    let db = build(&points, &[Road { from: 0, to: 1, m: Some(line) }, Road { from: 1, to: 2, m: None }]);
    let error = try_route(&db, 0, "n: 2", " by &m").unwrap_err();
    assert!(error.contains("road#2 from Junction#2 to Junction#3 has no m"), "{error}");
    assert!(error.contains("a path does not guess a missing weight"), "{error}");
    // Counting edges needs no weight.
    assert_eq!(route(&db, 0, "n: 2", "")["hops"], json!(2));
    // A negative weight.
    let db = build(&points, &[Road { from: 0, to: 1, m: Some(-5) }]);
    let error = try_route(&db, 0, "n: 1", " by &m").unwrap_err();
    assert!(error.contains("road#1 from Junction#1 to Junction#2 has m -5, and a path weight cannot be negative"), "{error}");
    // A weight shorter than the straight line: km stored, metres declared.
    let db = build(&points, &[Road { from: 0, to: 1, m: Some(line) }]);
    let error = try_route(&db, 0, "n: 1", " by &km toward at in m").unwrap_err();
    assert!(error.contains("shorter than the"), "{error}");
    assert!(error.contains("check the unit"), "{error}");
    assert_eq!(route(&db, 0, "n: 1", " by &km toward at in km")["hops"], json!(1));
}

#[test]
fn astar_needs_a_location_on_every_node_it_reaches() {
    let schema = "type Stop { n: Int at?: Point next -> Stop[] { m: Int } }";
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(schema, "mutation { Stop(n: 1 && at: point(51.0, -114.0)) { n } }").unwrap();
    db.run_lang(schema, "mutation { Stop(n: 2) { n } }").unwrap();
    db.run_lang(schema, "mutation { Stop(n: 1) { next -> link Stop(n: 2) { &m: 5000 } } }").unwrap();
    let error = db
        .run_lang(schema, "{ Stop(n: 1) { next *path by &m toward at in m -> Stop(n: 2) { n } } }")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Stop#2 has no at, and `toward at` needs a location on every node it reaches"), "{error}");
    let found = db.run_lang(schema, "{ Stop(n: 1) { next *path by &m -> Stop(n: 2) { n } } }").unwrap();
    assert_eq!(found["next"]["cost"], json!(5000));
}

#[test]
fn astar_needs_a_location_on_the_start_too() {
    // A start type without the Point: the checker refuses before any search.
    let schema = "type Depot { name: String road -> Junction[] { m: Int } }\ntype Junction { name: String at: Point road -> Junction[] { m: Int } }\n";
    let query = "{ Depot { road *path by &m toward at in m -> Junction { name } } }";
    let report = zega::diagnose(schema, query);
    let message = "Depot has no at, and `toward at` needs a location on every node it reaches";
    let diag = report
        .diagnostics
        .iter()
        .find(|diag| diag.message == message)
        .unwrap_or_else(|| panic!("{:?}", report.diagnostics));
    let start = diag.column as usize - 1;
    assert_eq!(&query[start..start + diag.underline_length as usize], "at");
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(schema, "mutation { Depot(name: \"D\") { road -> Junction(name: \"J\" && at: point(51.0, -114.0)) { name &m: 1 } } }")
        .unwrap();
    let error = db.run_lang(schema, query).unwrap_err().to_string();
    assert!(error.contains(message), "{error}");
    // Without `toward` the same route is fine.
    let found = db.run_lang(schema, "{ Depot { road *path by &m -> Junction { name } } }").unwrap();
    assert_eq!(found[0]["road"]["cost"], json!(1));

    // An optional Point missing on the start: the runtime refuses too, rather
    // than skipping the straight-line check on the roads leaving it (this
    // 1 m road is far shorter than its straight line).
    let schema = "type Stop { n: Int at?: Point next -> Stop[] { m: Int } }";
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(schema, "mutation { Stop(n: 1) { n } }").unwrap();
    db.run_lang(schema, "mutation { Stop(n: 2 && at: point(51.0, -114.0)) { n } }").unwrap();
    db.run_lang(schema, "mutation { Stop(n: 1) { next -> link Stop(n: 2) { &m: 1 } } }").unwrap();
    let error = db
        .run_lang(schema, "{ Stop(n: 1) { next *path by &m toward at in m -> Stop(n: 2) { n } } }")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Stop#1 has no at, and `toward at` needs a location on every node it reaches"), "{error}");
}

#[test]
fn the_checker_rejects_paths_it_cannot_run() {
    let schema = "type Junction { name: String at: Point road -> Junction[] { km: Float label?: String } go -> Place[] }\ntype Place { name: String }\n";
    let cases = [
        ("{ Junction { road *path by &kms -> Junction { name } } }", "road has no field kms", "&kms"),
        ("{ Junction { road *path by &label -> Junction { name } } }", "a path weight is a number; road.label is String", "&label"),
        ("{ Junction { road *path(hops <= 3) by &km -> Junction { name } } }", "a weighted path is bounded by cost", "hops <= 3"),
        ("{ Junction { road *path toward at in km -> Junction { name } } }", "toward needs a weight measured in a distance", "at"),
        ("{ Junction { road *path by &km toward name in km -> Junction { name } } }", "toward needs a Point; Junction.name is String", "name"),
        ("{ Junction { go *path -> Place { name } } }", "a path keeps following go, and Place has no `go ->`", "go"),
        ("mutation { Junction(name: \"A\") { road *path -> Junction(name: \"B\") { name } } }", "a mutation cannot find a path", "path"),
        ("{ Junction { road *path -> Junction limit 1 { name } } }", "a path target takes a condition, not near, order or limit", "Junction"),
    ];
    for (query, message, marked) in cases {
        let report = zega::diagnose(schema, query);
        let diag = report
            .diagnostics
            .iter()
            .find(|diag| diag.message == message)
            .unwrap_or_else(|| panic!("{query}: {:?}", report.diagnostics));
        let line = query.lines().nth(diag.line as usize - 1).unwrap();
        let start = diag.column as usize - 1;
        assert_eq!(&line[start..start + diag.underline_length as usize], marked, "{query}");
        // Execution stops at the same check.
        let db = Zega::in_memory().build().unwrap();
        let error = db.run_lang(schema, query).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
    }
    let parse = [
        ("{ Junction { road *path by &km toward at -> Junction { name } } }", "toward needs the unit of the weight"),
        ("{ Junction { road *path by &km toward at in feet -> Junction { name } } }", "unknown distance unit feet"),
        ("{ Junction { road *path(depth <= 3) -> Junction { name } } }", "unknown path bound depth"),
        ("{ Junction { road *path(hops < 0) -> Junction { name } } }", "`hops < 0` allows no route"),
        ("{ Junction { road *path(cost <= -1) -> Junction { name } } }", "a cost bound is a non-negative number"),
        ("{ Junction { road *path by km -> Junction { name } } }", "a path weight is an edge field"),
        ("{ Junction { road *path } }", "road *path needs an arrow and a target"),
    ];
    for (query, message) in parse {
        let report = zega::diagnose(schema, query);
        assert!(
            report.diagnostics.iter().any(|diag| diag.message == message),
            "{query}: {:?}",
            report.diagnostics
        );
    }
    // `*1..3` is unchanged beside `*path`.
    let report = zega::diagnose(schema, "{ Junction { road *1..3 -> Junction { name &hops } } }");
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

#[test]
fn the_traversal_budget_covers_path_searches() {
    let (points, roads) = grid(6);
    let small = build_with(&points, &roads, Zega::in_memory().traversal_work_budget(10).build().unwrap());
    let error = try_route(&small, 0, "n: 35", " by &m").unwrap_err();
    assert!(error.contains("traversal work budget exceeded"), "{error}");
    let db = build(&points, &roads);
    assert_eq!(route(&db, 0, "n: 35", " by &m")["hops"], json!(10));
}
