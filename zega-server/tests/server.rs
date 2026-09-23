use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};
use zega_core::Zega;
use zega_server::{server, AppState};

const TOKEN: &str = "test-secret";

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
        server::serve(listener, AppState::new(zega, TOKEN))
            .await
            .unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        _data: data,
        task,
    }
}

fn authed(client: &Client, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
    client.request(method, url).bearer_auth(TOKEN)
}

#[tokio::test]
async fn cql_create_then_match_returns_node() {
    let server = start_server().await;
    let client = Client::new();
    let create = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({"query": "CREATE (n:Person {name: 'Ada'})"}))
    .send()
    .await
    .unwrap();
    assert!(create.status().is_success());

    let body: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({"query": "MATCH (n:Person) RETURN n.name AS name"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["count"], 1);
    assert_eq!(body["rows"][0]["name"], "Ada");
}

#[tokio::test]
async fn cql_raw_json_params_round_trip() {
    let server = start_server().await;
    let client = Client::new();
    let params = json!({
        "string": "x",
        "int": 42,
        "float": 2.5,
        "bool": true,
        "null_value": null,
        "array": ["nested", 7, false, null],
        "object": {"name": "Ada", "scores": [1, 2.5]}
    });
    let body: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({
        "query": "CREATE (n:RawParams {string: $string, int: $int, float: $float, bool: $bool, null_value: $null_value, array: $array, object: $object})",
        "params": params
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["ok"], true, "{body}");

    let matched: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({
        "query": "MATCH (n:RawParams) RETURN n.string AS string, n.int AS int, n.float AS float, n.bool AS bool, n.null_value AS null_value, n.array AS array, n.object AS object"
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(matched["ok"], true);
    assert_eq!(matched["rows"][0]["string"], "x");
    assert_eq!(matched["rows"][0]["int"], 42);
    assert_eq!(matched["rows"][0]["float"], 2.5);
    assert_eq!(matched["rows"][0]["bool"], true);
    assert_eq!(matched["rows"][0]["null_value"], Value::Null);
    assert_eq!(
        matched["rows"][0]["array"],
        json!(["nested", 7, false, null])
    );
    assert_eq!(
        matched["rows"][0]["object"],
        json!({"name": "Ada", "scores": [1, 2.5]})
    );
}

#[tokio::test]
async fn create_with_raw_params_then_match_returns_raw_json() {
    let server = start_server().await;
    let client = Client::new();
    let create: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({
        "query": "CREATE (n:Person {name: $name, age: $age, active: $active})",
        "params": {"name": "Ada", "age": 37, "active": true}
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(create["ok"], true);

    let matched: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({
        "query": "MATCH (n:Person {name: $name}) RETURN n.name AS name, n.age AS age, n.active AS active",
        "params": {"name": "Ada"}
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        matched["rows"][0],
        json!({"name": "Ada", "age": 37, "active": true})
    );
}

#[tokio::test]
async fn health_is_ok_and_requires_token() {
    let server = start_server().await;
    let client = Client::new();
    let unauthorized = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.json::<Value>().await.unwrap()["ok"], false);

    let health: Value = authed(
        &client,
        reqwest::Method::GET,
        format!("{}/health", server.base_url),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(health["ok"], true);
}

#[tokio::test]
async fn malformed_query_is_json_error_not_server_failure() {
    let server = start_server().await;
    let client = Client::new();
    let response = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .json(&json!({"query": "MATCH this is not valid"}))
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"].is_string());

    let health = authed(
        &client,
        reqwest::Method::GET,
        format!("{}/health", server.base_url),
    )
    .send()
    .await
    .unwrap();
    assert!(health.status().is_success());
}

#[tokio::test]
async fn malformed_json_is_json_error() {
    let server = start_server().await;
    let client = Client::new();
    let response = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/cql", server.base_url),
    )
    .header("content-type", "application/json")
    .body("{")
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_readers_never_observe_half_applied_write() {
    let server = start_server().await;
    let client = Arc::new(Client::new());
    let base_url = Arc::new(server.base_url.clone());

    let mut tasks = Vec::new();
    for writer in 0..4 {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        tasks.push(tokio::spawn(async move {
            for pair in 0..20 {
                let query = format!(
                    "CREATE (a:GateTest {{id: 'a-{writer}-{pair}'}}); \
                     CREATE (b:GateTest {{id: 'b-{writer}-{pair}'}})"
                );
                let body: Value = authed(&client, reqwest::Method::POST, format!("{base_url}/cql"))
                    .json(&json!({"query": query}))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(body["ok"], true);
            }
        }));
    }
    for _ in 0..16 {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        tasks.push(tokio::spawn(async move {
            for _ in 0..50 {
                let body: Value = authed(&client, reqwest::Method::POST, format!("{base_url}/cql"))
                    .json(&json!({"query": "MATCH (n:GateTest) RETURN count(*) AS count"}))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(body["ok"], true);
                let count = body["rows"][0]["count"].as_i64().unwrap();
                assert_eq!(count % 2, 0, "reader observed a half-applied pair");
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}
