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
}
impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn start_server() -> TestServer {
    let data = tempfile::tempdir().unwrap();
    let zega = Zega::open(data.path().to_str().unwrap()).build().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        server::serve(listener, AppState::new(zega, Some(TOKEN)))
            .await
            .unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        _data: data,
        task,
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
