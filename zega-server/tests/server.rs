use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};
use zega::Zega;
use zega_server::{server, AppState};

const TOKEN: &str = "test-secret";
const SCHEMA: &str = "type Person { name: String age?: Int active?: Bool }";
struct TestServer {
    base_url: String,
    _data: TempDir,
    task: JoinHandle<()>,
    /// The server's database, behind the same gate its requests take.
    zega: std::sync::Arc<std::sync::Mutex<Zega>>,
}
impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn start_server() -> TestServer {
    start_server_with(zega_server::DEFAULT_MAX_IMPORT_BYTES, zega_server::DEFAULT_TRANSFER_IDLE_TIMEOUT).await
}

async fn start_server_with(max_import_bytes: u64, idle: std::time::Duration) -> TestServer {
    start_server_full(max_import_bytes, idle, zega_server::DEFAULT_TRANSFER_SLOTS).await
}

async fn start_server_full(max_import_bytes: u64, idle: std::time::Duration, slots: usize) -> TestServer {
    let data = tempfile::tempdir().unwrap();
    let zega = Zega::open(data.path().to_str().unwrap()).build().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = AppState::new(zega, Some(TOKEN))
        .with_import_limits(max_import_bytes, idle)
        .with_transfer_slots(slots);
    let zega = state.zega.clone();
    let task = tokio::spawn(async move {
        server::serve(listener, state).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        _data: data,
        task,
        zega,
    }
}
fn post(client: &Client, server: &TestServer) -> reqwest::RequestBuilder {
    client
        .post(format!("{}/zql", server.base_url))
        .bearer_auth(TOKEN)
}

#[tokio::test]
async fn zql_mutation_then_query_returns_typed_json() {
    let server = start_server().await;
    let client = Client::new();
    let body: Value = post(&client, &server).json(&json!({"schema":SCHEMA,"query":"mutation { Person(name: \"Ada\" && age: 37 && active: true) { name age active } }"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(
        body,
        json!({"ok":true,"result":{"name":"Ada","age":37,"active":true}})
    );
    let body: Value = post(&client, &server)
        .json(&json!({"schema":SCHEMA,"query":"{ Person { name age active } }"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        body["result"],
        json!([{"name":"Ada","age":37,"active":true}])
    );
    assert_eq!(
        client
            .post(format!("{}/cql", server.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn vector_view_endpoint_uses_native_vectors_for_both_dimensions() {
    let server = start_server().await;
    let client = Client::new();
    let schema = "type Ticket { title: String embedding: Vector<2> } display { vector2d { Ticket }: Default vector3d { Ticket } }";
    for mutation in [
        "mutation { Ticket(title: \"A\" && embedding: @vector[1,0]) { @id } }",
        "mutation { Ticket(title: \"B\" && embedding: @vector[0.9,0.1]) { @id } }",
        "mutation { Ticket(title: \"C\" && embedding: @vector[-1,0]) { @id } }",
    ] {
        post(&client, &server)
            .json(&json!({"schema":schema,"query":mutation}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    let query: Value = post(&client, &server)
        .json(&json!({"schema":schema,"query":"{ Ticket { @id title } }"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for kind in ["vector2d", "vector3d"] {
        let view: Value = client
            .post(format!("{}/vector-view", server.base_url))
            .bearer_auth(TOKEN)
            .json(&json!({
                "schema":schema,
                "result":query["result"],
                "kind":kind,
                "selected":null,
                "k":10,
                "threshold":0.8
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(view["result"]["points"].as_array().unwrap().len(), 3);
        assert!(view["result"]["points"]
            .as_array()
            .unwrap()
            .iter()
            .all(|point| point["dimensions"] == 2));
    }
}

#[tokio::test]
async fn raw_import_uses_the_engine_and_graph_edits_use_wal_paths() {
    let server = start_server().await;
    let client = Client::new();
    let body: Value = post(&client, &server).json(&json!({"schema":SCHEMA,"query":"mutation csv [\"./import\"] { Person(name: $Name && age: $Age) { name age } }", "sources":{"./import":"Name,Age\nAda,37\n"}})).send().await.unwrap().json().await.unwrap();
    assert_eq!(body["result"], json!([{"name":"Ada","age":37}]));
    let graph: Value = client
        .get(format!("{}/graph", server.base_url))
        .header("accept", "application/json")
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = graph["result"]["nodes"][0]["id"].as_u64().unwrap();
    assert!(client
        .delete(format!("{}/graph/nodes/{id}", server.base_url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let body: Value = post(&client, &server)
        .json(&json!({"schema":SCHEMA,"query":"{ Person { name } }"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["result"], json!([]));
}

#[tokio::test]
async fn every_database_route_requires_the_bearer() {
    let server = start_server().await;
    let client = Client::new();
    for (method, path) in [
        (reqwest::Method::GET, "/health"),
        (reqwest::Method::POST, "/zql"),
        (reqwest::Method::POST, "/vector-view"),
        (reqwest::Method::GET, "/graph"),
        (reqwest::Method::PUT, "/graph"),
        (reqwest::Method::DELETE, "/graph"),
        (reqwest::Method::DELETE, "/graph/nodes/1"),
        (reqwest::Method::DELETE, "/graph/relationships/1"),
        (reqwest::Method::POST, "/graph/relationships"),
    ] {
        for token in [None, Some("wrong")] {
            let request = client.request(method.clone(), format!("{}{path}", server.base_url));
            let request = if let Some(token) = token {
                request.bearer_auth(token)
            } else {
                request
            };
            assert_eq!(
                request.json(&json!({})).send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
    }
    let body: Value = client
        .get(format!("{}/health", server.base_url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!({"ok":true}));
}

#[tokio::test]
async fn malformed_zql_is_json_error_and_server_stays_healthy() {
    let server = start_server().await;
    let client = Client::new();
    let response = post(&client, &server)
        .json(&json!({"schema":SCHEMA,"query":"MATCH this is not ZQL"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(response.json::<Value>().await.unwrap()["error"].is_string());
    assert!(client
        .get(format!("{}/health", server.base_url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
}

/// zegadb/zega#48: each payload once aborted the whole process from a worker
/// thread. Now each is a 400, and the same server answers the next request.
#[tokio::test]
async fn deeply_nested_zql_is_a_400_and_the_server_keeps_serving() {
    let server = start_server().await;
    let client = Client::new();
    let schema = "type Item { key: String c?: Int links -> Item[] }";
    let chain: Vec<String> = (0..5_000).map(|n| format!("c = {n}")).collect();
    let deep = [
        format!("{{ Item({}key: \"s1\"{}) {{ key }} }}", "(".repeat(2_000), ")".repeat(2_000)),
        format!("{{ Item(key: \"s1\") {{ {}key{} }} }}", "links -> Item { ".repeat(1_000), " }".repeat(1_000)),
        format!(
            "mutation {{ Item(key: \"s0\") {{ {}key{} }} }}",
            (1..=1_000).map(|n| format!("links -> Item(key: \"s{n}\") {{ ")).collect::<String>(),
            " }".repeat(1_000)
        ),
    ];
    for query in &deep {
        let response = post(&client, &server)
            .json(&json!({"schema": schema, "query": query}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("nested too deeply (limit 128)"),
            "{body}"
        );
        let document = format!("schema {{ {schema} }}\n{query}");
        let response = post(&client, &server)
            .json(&json!({"query": document, "document": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    // A flat 5,000-term chain is not nesting: it runs.
    let query = format!("{{ Item({}) {{ key }} }}", chain.join(" || "));
    let body: Value = post(&client, &server)
        .json(&json!({"schema": schema, "query": query}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!({"ok": true, "result": []}));
    let body: Value = post(&client, &server)
        .json(&json!({"schema": schema, "query": "mutation { Item(key: \"after\") { key } }"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!({"ok": true, "result": {"key": "after"}}));
}

#[tokio::test]
async fn malformed_json_is_a_json_error() {
    let server = start_server().await;
    let response = post(&Client::new(), &server)
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>().await.unwrap()["ok"], false);
}

#[tokio::test]
async fn server_refuses_unauthenticated_non_loopback_listener() {
    let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
    let state = AppState::new(Zega::in_memory().build().unwrap(), None);
    assert_eq!(
        server::serve(listener, state).await.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readers_never_observe_half_a_zql_document() {
    let server = start_server().await;
    let mut tasks = Vec::new();
    for writer in 0..4 {
        let url = format!("{}/zql", server.base_url);
        tasks.push(tokio::spawn(async move {
            let client = Client::new();
            for pair in 0..10 {
                let source = format!("schema {{ type Pair {{ id: Int }} }} mutation {{ Pair(id: {}) {{ id }} }} mutation {{ Pair(id: {}) {{ id }} }}", writer*100+pair*2,writer*100+pair*2+1);
                let body: Value = client.post(&url).bearer_auth(TOKEN).json(&json!({"document":true,"query":source})).send().await.unwrap().json().await.unwrap();
                assert_eq!(body["ok"],true,"{body}");
            }
        }));
    }
    for _ in 0..8 {
        let url = format!("{}/zql", server.base_url);
        tasks.push(tokio::spawn(async move {
            let client = Client::new();
            for _ in 0..25 {
                let body: Value = client
                    .post(&url)
                    .bearer_auth(TOKEN)
                    .json(&json!({"schema":"type Pair { id: Int }","query":"{ Pair { id } }"}))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(
                    body["result"].as_array().unwrap().len() % 2,
                    0,
                    "partial pair: {body}"
                );
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}

/// zegadb/zega#63: the server's own limit, as `zega start` sets it. The
/// traversal budget is lifted so that the limit, not the budget, is what
/// stops the slow reads below.
async fn start_limited_server() -> TestServer {
    let data = tempfile::tempdir().unwrap();
    let zega = Zega::open(data.path().to_str().unwrap())
        .traversal_work_budget(usize::MAX)
        .query_time_limit(zega_server::DEFAULT_QUERY_TIME_LIMIT)
        .build()
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = AppState::new(zega, Some(TOKEN));
    let zega = state.zega.clone();
    let task = tokio::spawn(async move {
        server::serve(listener, state).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        _data: data,
        task,
        zega,
    }
}

const STOPS: &str = "type Stop { name: String at: Point seen?: Int }";

/// `n` stops at one place, loaded through the server.
async fn load_stops(client: &Client, server: &TestServer, n: usize) {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"name":"s{i}","at":{{"lat":51.05,"lon":-114.07}}}}"#))
        .collect();
    let body: Value = post(client, server)
        .json(&json!({
            "schema": STOPS,
            "query": r#"mutation json ["stops.json"] { Stop(name: $name && at: $at) }"#,
            "sources": {"stops.json": format!("[{}]", rows.join(","))},
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["ok"], true, "{body}");
}

/// Every pair of 4,000 stops: about 19 s in a debug build, unbounded.
const PAIRS: &str = "query { Stop } display { skip } then { near { &at < 0 m } }";

fn assert_time_limit(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body,
        &json!({"ok": false, "error": "query exceeded the 2 s limit", "code": "query_time_limit"})
    );
}

#[tokio::test]
async fn a_slow_query_gets_the_time_limit_error_at_about_two_seconds() {
    let server = start_limited_server().await;
    let client = Client::new();
    load_stops(&client, &server, 4_000).await;
    let started = std::time::Instant::now();
    let response = post(&client, &server)
        .json(&json!({"schema": STOPS, "query": PAIRS}))
        .send()
        .await
        .unwrap();
    let took = started.elapsed();
    let status = response.status();
    assert_time_limit(status, &response.json().await.unwrap());
    assert!(
        took >= std::time::Duration::from_secs(2) && took < std::time::Duration::from_secs(4),
        "answered after {took:?}"
    );
}

#[tokio::test]
async fn a_mutation_that_hits_the_time_limit_writes_nothing() {
    let server = start_limited_server().await;
    let client = Client::new();
    load_stops(&client, &server, 8_000).await;
    // Each row finds its stop by an unindexed name: every row scans every stop.
    let rows: Vec<String> = (0..8_000).map(|i| format!(r#"{{"n":{i},"stop":"s{i}"}}"#)).collect();
    let response = post(&client, &server)
        .json(&json!({
            "schema": STOPS,
            "query": r#"mutation json ["seen.json"] { Stop(name = $stop) set seen: $n }"#,
            "sources": {"seen.json": format!("[{}]", rows.join(","))},
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    assert_time_limit(status, &response.json().await.unwrap());
    let body: Value = post(&client, &server)
        .json(&json!({"schema": STOPS, "query": "query { Stop(seen >= 0) { name } }"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!({"ok": true, "result": []}));
}

#[tokio::test]
async fn fast_requests_keep_answering_while_a_slow_one_times_out() {
    let server = start_limited_server().await;
    let client = Client::new();
    load_stops(&client, &server, 4_000).await;
    let started = std::time::Instant::now();
    let slow = tokio::spawn({
        let request = post(&client, &server).json(&json!({"schema": STOPS, "query": PAIRS}));
        async move {
            let response = request.send().await.unwrap();
            (response.status(), response.json::<Value>().await.unwrap())
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // Health never waits for the database.
    let health = client
        .get(format!("{}/health", server.base_url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success());
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    // Queries wait for the one database lock, so they answer once the slow
    // query has been stopped: within the limit, not after the full 19 s.
    let fast: Vec<_> = (0..5)
        .map(|i| {
            let request = post(&client, &server).json(&json!({
                "schema": STOPS,
                "query": format!("query {{ Stop(name: \"s{i}\") {{ name }} }}"),
            }));
            tokio::spawn(async move { request.send().await.unwrap().json::<Value>().await.unwrap() })
        })
        .collect();
    for (i, answer) in fast.into_iter().enumerate() {
        assert_eq!(
            answer.await.unwrap(),
            json!({"ok": true, "result": {"name": format!("s{i}")}})
        );
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(4), "{:?}", started.elapsed());
    let (status, body) = slow.await.unwrap();
    assert_time_limit(status, &body);
}

#[tokio::test]
async fn a_query_inside_the_limit_is_not_affected() {
    let server = start_limited_server().await;
    let client = Client::new();
    load_stops(&client, &server, 300).await;
    let body: Value = post(&client, &server)
        .json(&json!({"schema": STOPS, "query": PAIRS}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["ok"], true, "{body}");
}

// ---------------------------------------------------------------------------
// `.graph` over HTTP: `GET /graph` streams it, `PUT /graph` imports it.

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../zega/tests/fixtures/golden-v1.graph");

async fn download(client: &Client, server: &TestServer) -> Vec<u8> {
    let response = client
        .get(format!("{}/graph", server.base_url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        zega::graph_file::MEDIA_TYPE
    );
    response.bytes().await.unwrap().to_vec()
}

async fn upload(client: &Client, server: &TestServer, bytes: Vec<u8>) -> (StatusCode, Value) {
    let response = client
        .put(format!("{}/graph", server.base_url))
        .bearer_auth(TOKEN)
        .header("content-type", zega::graph_file::MEDIA_TYPE)
        .body(bytes)
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}



#[tokio::test]
async fn get_graph_streams_the_graph_file_the_engine_exports() {
    let server = start_server().await;
    let client = Client::new();
    post(&client, &server)
        .json(&json!({"schema":SCHEMA,"query":"mutation { Person(name: \"Ada\" && age: 37) { name } }"}))
        .send()
        .await
        .unwrap();
    let bytes = download(&client, &server).await;
    assert!(bytes.starts_with(&zega::graph_file::MAGIC));
    let local = Zega::in_memory().build().unwrap();
    let summary = local.import(&bytes[..]).unwrap();
    assert_eq!((summary.nodes, summary.relationships), (1, 0));
    let mut again = Vec::new();
    local.export(&mut again).unwrap();
    assert_eq!(again, bytes, "the download is exactly what the engine exports");
}

#[tokio::test]
async fn put_graph_replaces_the_graph_and_get_returns_it() {
    let server = start_server().await;
    let client = Client::new();
    let golden = std::fs::read(GOLDEN).unwrap();
    let (status, body) = upload(&client, &server, golden.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["nodes"], 3);
    assert_eq!(body["result"]["relationships"], 2);
    assert_eq!(body["result"]["meta"]["licence"], "CC0-1.0");
    // Import then export is the same file, schema and metadata included.
    assert_eq!(download(&client, &server).await, golden);
}

#[tokio::test]
async fn put_graph_refuses_a_damaged_file_and_changes_nothing() {
    let server = start_server().await;
    let client = Client::new();
    post(&client, &server)
        .json(&json!({"schema":SCHEMA,"query":"mutation { Person(name: \"Ada\" && age: 37) { name } }"}))
        .send()
        .await
        .unwrap();
    let before = download(&client, &server).await;
    let golden = std::fs::read(GOLDEN).unwrap();
    let mut flipped = golden.clone();
    let at = flipped.len() / 2;
    flipped[at] ^= 0x01;
    for (bytes, message) in [
        (golden[..golden.len() - 1].to_vec(), "truncated .graph file"),
        (flipped, "corrupt .graph file"),
        (b"not a graph".to_vec(), "not a .graph file"),
    ] {
        let (status, body) = upload(&client, &server, bytes).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().starts_with(message), "{body}");
        assert_eq!(download(&client, &server).await, before);
    }
}

/// `PUT /graph` streams: a file past the 16 MB JSON body limit imports.
#[tokio::test]
async fn put_graph_accepts_a_file_larger_than_the_json_body_limit() {
    let source = Zega::in_memory().build().unwrap();
    let big = "x".repeat(1_000_000);
    for _ in 0..18 {
        source
            .run_lang(SCHEMA, &format!("mutation {{ Person(name: \"{big}\") {{ name }} }}"))
            .unwrap();
    }
    let mut bytes = Vec::new();
    source.export(&mut bytes).unwrap();
    assert!(bytes.len() > 16_000_000);
    let server = start_server().await;
    let client = Client::new();
    let (status, body) = upload(&client, &server, bytes.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(download(&client, &server).await, bytes);
}

#[tokio::test]
async fn get_graph_negotiates_on_accept_with_q_values_and_says_it_varies() {
    let server = start_server().await;
    let client = Client::new();
    for (accept, expected) in [
        (None, Some("application/vnd.zega.graph")),
        (Some("*/*"), Some("application/vnd.zega.graph")),
        (Some("application/json"), Some("application/json")),
        (Some("application/json, */*;q=0.1"), Some("application/json")),
        (Some("application/json;q=0.5, application/vnd.zega.graph"), Some("application/vnd.zega.graph")),
        (Some("application/vnd.zega.graph;q=0.2, application/*;q=0.9"), Some("application/json")),
        (Some("application/json;q=0, */*"), Some("application/vnd.zega.graph")),
        (Some("text/html"), None),
    ] {
        let request = client.get(format!("{}/graph", server.base_url)).bearer_auth(TOKEN);
        let request = match accept {
            Some(accept) => request.header("accept", accept),
            None => request,
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.headers()["vary"], "accept", "{accept:?}");
        match expected {
            Some(kind) => {
                assert_eq!(response.status(), StatusCode::OK, "{accept:?}");
                assert!(response.headers()["content-type"].to_str().unwrap().starts_with(kind), "{accept:?}");
            }
            None => assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{accept:?}"),
        }
    }
}

/// Send the start of a request on a raw socket and keep the socket open
/// without sending (or reading) anything more: a stalled client.
async fn stalled(server: &TestServer, head: String, body: &[u8]) -> tokio::net::TcpStream {
    use tokio::io::AsyncWriteExt;
    let address = server.base_url.trim_start_matches("http://");
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(head.as_bytes()).await.unwrap();
    socket.write_all(body).await.unwrap();
    socket
}

async fn quick_query(client: &Client, server: &TestServer) -> std::time::Duration {
    let start = std::time::Instant::now();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        // A query that needs the database gate but reads nothing.
        post(client, server).json(&json!({"schema":"type Other { name: String }","query":"{ Other { name } }"})).send(),
    )
    .await
    .expect("a query waited behind a stalled .graph transfer")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    start.elapsed()
}

/// A graph whose file is far larger than every socket and channel buffer
/// between the server and a client that stops reading.
async fn big_graph(client: &Client, server: &TestServer) {
    let big = "x".repeat(1_000_000);
    for _ in 0..40 {
        post(client, server)
            .json(&json!({"schema":SCHEMA,"query":format!("mutation {{ Person(name: \"{big}\") {{ name }} }}")}))
            .send()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_download_nobody_reads_does_not_block_queries() {
    let server = start_server().await;
    let client = Client::new();
    big_graph(&client, &server).await;
    let mut reader = stalled(
        &server,
        format!("GET /graph HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"),
        b"",
    )
    .await;
    // Read the response head (the export is written by then), then stop
    // reading: 40 MB of body back up behind this client.
    use tokio::io::AsyncReadExt;
    let mut head = [0u8; 12];
    reader.read_exact(&mut head).await.unwrap();
    assert_eq!(&head, b"HTTP/1.1 200");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let took = quick_query(&client, &server).await;
    assert!(took < std::time::Duration::from_secs(1), "{took:?}");
}

#[tokio::test]
async fn a_stalled_upload_does_not_block_queries_and_times_out() {
    let server = start_server_with(1 << 30, std::time::Duration::from_secs(2)).await;
    let client = Client::new();
    post(&client, &server)
        .json(&json!({"schema":SCHEMA,"query":"mutation { Person(name: \"Ada\") { name } }"}))
        .send()
        .await
        .unwrap();
    let before = download(&client, &server).await;
    let golden = std::fs::read(GOLDEN).unwrap();
    let mut socket = stalled(
        &server,
        format!(
            "PUT /graph HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: {}\r\n\r\n",
            golden.len()
        ),
        &golden[..100],
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let took = quick_query(&client, &server).await;
    assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    // After the idle timeout the upload is refused and nothing changed.
    use tokio::io::AsyncReadExt;
    let mut response = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(10), socket.read_to_end(&mut response))
        .await
        .expect("the stalled upload was never answered")
        .unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 408"), "{response}");
    assert!(response.contains("the upload stalled"), "{response}");
    assert_eq!(download(&client, &server).await, before);
}

#[tokio::test]
async fn an_upload_over_the_size_limit_is_refused() {
    let golden = std::fs::read(GOLDEN).unwrap();
    let server = start_server_with(golden.len() as u64 - 1, zega_server::DEFAULT_TRANSFER_IDLE_TIMEOUT).await;
    let client = Client::new();
    let (status, body) = upload(&client, &server, golden.clone()).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert!(body["error"].as_str().unwrap().contains("--max-import-bytes"), "{body}");
    let server = start_server_with(golden.len() as u64, zega_server::DEFAULT_TRANSFER_IDLE_TIMEOUT).await;
    let (status, body) = upload(&client, &server, golden).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn transfers_leave_no_staging_files() {
    let server = start_server().await;
    let client = Client::new();
    let golden = std::fs::read(GOLDEN).unwrap();
    upload(&client, &server, golden.clone()).await;
    upload(&client, &server, golden[..50].to_vec()).await;
    download(&client, &server).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut names: Vec<String> = std::fs::read_dir(server._data.path().join("graphs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].ends_with(".graph") && !names[0].starts_with('.'), "{names:?}");
}

/// The staging files of `GET /graph` downloads, and nothing else in `graphs/`
/// (an upload's staging file, a checkpoint's file in flight).
fn export_staging_files(server: &TestServer) -> Vec<String> {
    staging_files(server).into_iter().filter(|name| name.starts_with(".export-")).collect()
}

fn staging_files(server: &TestServer) -> Vec<String> {
    match std::fs::read_dir(server._data.path().join("graphs")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn delete_graph_drops_what_an_import_carried() {
    let server = start_server().await;
    let client = Client::new();
    let (status, _) = upload(&client, &server, std::fs::read(GOLDEN).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let response = client
        .delete(format!("{}/graph", server.base_url))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let local = Zega::in_memory().build().unwrap();
    let summary = local.import(&download(&client, &server).await[..]).unwrap();
    assert_eq!((summary.nodes, summary.relationships), (0, 0));
    assert_eq!(summary.schema, None);
    assert!(summary.meta.is_empty(), "the old licence survived: {:?}", summary.meta);
}

/// A client that stops reading a download is dropped after the idle
/// timeout, and its staging file goes with it.
#[tokio::test]
async fn a_stalled_download_is_dropped_and_its_staging_file_deleted() {
    let server = start_server_with(zega_server::DEFAULT_MAX_IMPORT_BYTES, std::time::Duration::from_secs(1)).await;
    let client = Client::new();
    big_graph(&client, &server).await;
    let mut reader = stalled(
        &server,
        format!("GET /graph HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"),
        b"",
    )
    .await;
    use tokio::io::AsyncReadExt;
    let mut head = [0u8; 12];
    reader.read_exact(&mut head).await.unwrap();
    assert_eq!(&head, b"HTTP/1.1 200");
    assert_eq!(export_staging_files(&server).len(), 1, "the export is staged while it is sent");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !export_staging_files(&server).is_empty() {
        assert!(std::time::Instant::now() < deadline, "the stalled download kept {:?}", export_staging_files(&server));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    drop(reader);
}

/// Transfers take a slot each; with none free the server says 503 rather
/// than stage another file, and a slot comes back when its transfer ends.
#[tokio::test]
async fn transfers_beyond_the_slots_get_503() {
    let server = start_server_full(zega_server::DEFAULT_MAX_IMPORT_BYTES, std::time::Duration::from_secs(1), 1).await;
    let client = Client::new();
    let golden = std::fs::read(GOLDEN).unwrap();
    let mut socket = stalled(
        &server,
        format!(
            "PUT /graph HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: {}\r\n\r\n",
            golden.len()
        ),
        &golden[..100],
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (status, body) = upload(&client, &server, golden.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let busy = client.get(format!("{}/graph", server.base_url)).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    use tokio::io::AsyncReadExt;
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 408"));
    let (status, body) = upload(&client, &server, golden).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A declared length over the cap is refused before any of the body is
/// read or staged.
#[tokio::test]
async fn an_oversized_content_length_is_refused_up_front() {
    let server = start_server().await;
    let too_big = zega_server::DEFAULT_MAX_IMPORT_BYTES + 1;
    let mut socket = stalled(
        &server,
        format!("PUT /graph HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: {too_big}\r\n\r\n"),
        b"",
    )
    .await;
    use tokio::io::AsyncReadExt;
    let mut head = [0u8; 12];
    tokio::time::timeout(std::time::Duration::from_secs(3), socket.read_exact(&mut head))
        .await
        .expect("no early answer")
        .unwrap();
    assert_eq!(&head, b"HTTP/1.1 413");
    assert!(staging_files(&server).is_empty());
    assert_eq!(zega_server::DEFAULT_MAX_IMPORT_BYTES, 64 * 1024 * 1024);
}

/// zega#112 CI: a checkpoint taken while a download is stalled. The export's
/// staging file and the checkpoint's file sit in `graphs/` together; neither
/// is deleted or counted as the other, the checkpoint succeeds, and the
/// download, once read, is the whole graph.
#[tokio::test]
async fn a_checkpoint_while_a_download_is_stalled_leaves_both_whole() {
    use tokio::io::AsyncReadExt;
    let server = start_server_with(zega_server::DEFAULT_MAX_IMPORT_BYTES, std::time::Duration::from_secs(60)).await;
    let client = Client::new();
    big_graph(&client, &server).await;
    let mut reader = stalled(
        &server,
        format!("GET /graph HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"),
        b"",
    )
    .await;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let length: usize = head
        .lines()
        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(|n| n.trim().parse().unwrap()))
        .expect("a staged download has a length");
    let staged = export_staging_files(&server);
    assert_eq!(staged.len(), 1, "the export is staged while it is sent");

    let zega = server.zega.clone();
    let checkpoint = tokio::task::spawn_blocking(move || zega.lock().unwrap().checkpoint())
        .await
        .unwrap()
        .expect("a checkpoint beside a staged export failed")
        .unwrap();
    assert!(server._data.path().join(&checkpoint.file).exists());
    assert_eq!(export_staging_files(&server), staged, "the checkpoint touched the staged export");

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await.unwrap();
    let copy = Zega::in_memory().build().unwrap();
    let summary = copy.import(&body[..]).expect("the download is not a whole .graph file");
    assert_eq!(summary.nodes, 40);
}
