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

async fn post_kv(client: &Client, base_url: &str, body: Value) -> Value {
    authed(client, reqwest::Method::POST, format!("{base_url}/kv"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
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
async fn kv_set_then_get_round_trips() {
    let server = start_server().await;
    let client = Client::new();
    let set: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/kv", server.base_url),
    )
    .json(&json!({"op": "set", "key": "name", "value": "Ada"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(set["ok"], true);

    let get: Value = authed(
        &client,
        reqwest::Method::POST,
        format!("{}/kv", server.base_url),
    )
    .json(&json!({"op": "get", "key": "name"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(get["result"], "Ada");
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
async fn kv_set_accepts_explicit_raw_null() {
    let server = start_server().await;
    let client = Client::new();
    let set = post_kv(
        &client,
        &server.base_url,
        json!({"op": "set", "key": "nothing", "value": null}),
    )
    .await;
    assert_eq!(set["ok"], true);

    let get = post_kv(
        &client,
        &server.base_url,
        json!({"op": "get", "key": "nothing"}),
    )
    .await;
    assert_eq!(get["ok"], true);
    assert_eq!(get["result"], Value::Null);
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
async fn kv_list_counter_and_expiry_operations_work() {
    let server = start_server().await;
    let client = Client::new();

    let first = post_kv(
        &client,
        &server.base_url,
        json!({"op": "lpush", "key": "items", "value": 1}),
    )
    .await;
    assert_eq!(first["result"], 1);
    post_kv(
        &client,
        &server.base_url,
        json!({"op": "lpush", "key": "items", "value": 2}),
    )
    .await;
    let range = post_kv(
        &client,
        &server.base_url,
        json!({"op": "lrange", "key": "items", "start": 0, "stop": 10}),
    )
    .await;
    assert_eq!(range["result"], json!([2, 1]));
    post_kv(
        &client,
        &server.base_url,
        json!({"op": "ltrim", "key": "items", "start": 0, "stop": 1}),
    )
    .await;
    let exists = post_kv(
        &client,
        &server.base_url,
        json!({"op": "exists", "key": "items"}),
    )
    .await;
    assert_eq!(exists["result"], true);
    post_kv(
        &client,
        &server.base_url,
        json!({"op": "expire", "key": "items", "ttl": 60}),
    )
    .await;
    let ttl = post_kv(
        &client,
        &server.base_url,
        json!({"op": "ttl", "key": "items"}),
    )
    .await;
    assert!(ttl["result"].as_u64().unwrap() <= 60);
    let incremented = post_kv(
        &client,
        &server.base_url,
        json!({"op": "incr", "key": "counter"}),
    )
    .await;
    assert_eq!(incremented["result"], 1);
    let deleted = post_kv(
        &client,
        &server.base_url,
        json!({"op": "del", "key": "counter"}),
    )
    .await;
    assert_eq!(deleted["result"], true);
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

#[tokio::test]
async fn malformed_list_ranges_are_json_errors_not_server_failures() {
    let server = start_server().await;
    let client = Client::new();
    post_kv(
        &client,
        &server.base_url,
        json!({"op": "lpush", "key": "items", "value": 1}),
    )
    .await;

    for body in [
        json!({"op": "lrange", "key": "items", "start": 1, "stop": 0}),
        json!({"op": "ltrim", "key": "items", "start": usize::MAX, "stop": 0}),
        json!({"op": "lrange", "key": "items", "start": -1, "stop": 0}),
    ] {
        let response = authed(
            &client,
            reqwest::Method::POST,
            format!("{}/kv", server.base_url),
        )
        .json(&body)
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>().await.unwrap()["ok"], false);
    }

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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_expiry_readers_and_writers_remain_consistent() {
    let server = start_server().await;
    let client = Arc::new(Client::new());
    let base_url = Arc::new(server.base_url.clone());

    let writer_client = Arc::clone(&client);
    let writer_url = Arc::clone(&base_url);
    let writer = tokio::spawn(async move {
        for value in 0..200 {
            for body in [
                json!({"op": "set", "key": "value", "value": value, "ttl": 0}),
                json!({"op": "set", "key": "list", "value": [value], "ttl": 0}),
                json!({"op": "set", "key": "value", "value": value}),
                json!({"op": "set", "key": "list", "value": [value]}),
            ] {
                assert_eq!(post_kv(&writer_client, &writer_url, body).await["ok"], true);
            }
        }
    });

    let mut readers = Vec::new();
    for _ in 0..16 {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        readers.push(tokio::spawn(async move {
            for _ in 0..200 {
                let get = post_kv(&client, &base_url, json!({"op": "get", "key": "value"})).await;
                assert_eq!(get["ok"], true);
                assert!(get["result"].is_null() || get["result"].is_number());

                let range = post_kv(
                    &client,
                    &base_url,
                    json!({"op": "lrange", "key": "list", "start": 0, "stop": 1}),
                )
                .await;
                assert_eq!(range["ok"], true);
                let result = &range["result"];
                assert!(
                    result.is_null()
                        || result
                            .as_array()
                            .is_some_and(|items| items.len() == 1 && items[0].is_number())
                );
            }
        }));
    }

    writer.await.unwrap();
    for reader in readers {
        reader.await.unwrap();
    }
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
