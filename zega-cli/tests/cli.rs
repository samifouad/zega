use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const BIN: &str = env!("CARGO_BIN_EXE_zega");
const SCHEMA: &str = "type Player { name: String salary: Int }";
struct Running {
    child: Child,
    url: String,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Running {
    fn start(command: &str, directory: &Path, extra: &[&str]) -> Self {
        let child = Command::new(BIN)
            .arg(command)
            .args(["--port", "0", "--data"])
            .arg(directory.join("db"))
            .args(extra)
            .current_dir(directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            url: String::new(),
        };
        let stdout = server.child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let (read, line) = receive
            .recv_timeout(Duration::from_secs(15))
            .expect("CLI did not print its listening URL");
        assert!(read.unwrap() > 0, "CLI exited before startup");
        server.url = line.trim().to_string();
        assert!(
            server.url.starts_with("http://127.0.0.1:"),
            "{}",
            server.url
        );
        server
    }
    fn zql(&self, query: &str) -> Value {
        let (status, body, _) = self.request(
            "POST",
            "/zql",
            Some(json!({"schema":SCHEMA,"query":query})),
            None,
        );
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice::<Value>(&body).unwrap()["result"].clone()
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: Option<&str>,
    ) -> (u16, Vec<u8>, String) {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .proxy(None)
            .build()
            .new_agent();
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.url));
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let payload = body.map(|body| serde_json::to_vec(&body).unwrap());
        if payload.is_some() {
            request = request.header("Content-Type", "application/json");
        }
        let mut response = agent
            .run(request.body(payload.unwrap_or_default()).unwrap())
            .unwrap();
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|value| value.to_str().unwrap().to_string())
            .unwrap_or_default();
        (
            status,
            response.body_mut().read_to_vec().unwrap(),
            content_type,
        )
    }
}

#[test]
fn start_loads_local_json_and_persists_across_process_restart() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("players.json"),
        r#"[{"Name":"Ada","Salary":42}]"#,
    )
    .unwrap();
    {
        let server = Running::start("start", directory.path(), &[]);
        assert_eq!(server.zql("mutation json [\"./players.json\"] { Player(name: $Name && salary: $Salary) { name salary } }"),json!([{"name":"Ada","salary":42}]));
        assert_eq!(
            server.zql("{ Player { name salary } }"),
            json!([{"name":"Ada","salary":42}])
        );
    }
    std::fs::remove_file(directory.path().join("players.json")).unwrap();
    let server = Running::start("start", directory.path(), &[]);
    assert_eq!(
        server.zql("{ Player { name salary } }"),
        json!([{"name":"Ada","salary":42}])
    );
    assert_eq!(server.request("POST", "/cql", None, None).0, 404);
}

#[test]
fn token_file_requires_bearer_and_bad_configuration_fails() {
    let directory = tempfile::tempdir().unwrap();
    let token = directory.path().join("token");
    std::fs::write(&token, "test-token\n").unwrap();
    let server = Running::start(
        "start",
        directory.path(),
        &["--token-file", token.to_str().unwrap()],
    );
    for bearer in [None, Some("wrong")] {
        assert_eq!(server.request("GET", "/health", None, bearer).0, 401);
        assert_eq!(
            server
                .request(
                    "POST",
                    "/zql",
                    Some(json!({"schema":SCHEMA,"query":"{ Player { name } }"})),
                    bearer
                )
                .0,
            401
        );
    }
    assert_eq!(
        server.request("GET", "/health", None, Some("test-token")).0,
        200
    );
    assert_eq!(
        server
            .request(
                "POST",
                "/zql",
                Some(json!({"schema":SCHEMA,"query":"{ Player { name } }"})),
                Some("test-token")
            )
            .0,
        200
    );
    for args in [
        vec!["start", "--host", "0.0.0.0", "--port", "0"],
        vec!["start", "--token-file", "missing-token", "--port", "0"],
    ] {
        let output = Command::new(BIN)
            .args(args)
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    std::fs::write(&token, "\n").unwrap();
    let output = Command::new(BIN)
        .args([
            "start",
            "--token-file",
            token.to_str().unwrap(),
            "--port",
            "0",
        ])
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("test-token"));
}

#[test]
fn explorer_embeds_assets_and_shares_the_persistent_database() {
    let directory = tempfile::tempdir().unwrap();
    {
        let server = Running::start("explorer", directory.path(), &[]);
        let (status, body, mime) = server.request("GET", "/", None, None);
        assert_eq!(status, 200);
        assert!(mime.starts_with("text/html"));
        assert!(String::from_utf8(body)
            .unwrap()
            .contains("<title>zega</title>"));
        let (status, body, mime) = server.request("GET", "/pkg/zega_wasm_bg.wasm", None, None);
        assert_eq!(status, 200);
        assert_eq!(mime, "application/wasm");
        assert_eq!(&body[..4], b"\0asm");
        assert_eq!(server.request("GET", "/backend.js", None, None).0, 200);
        for path in ["/map.js", "/map-style.js", "/table.js", "/theme.js"] {
            let (status, _, mime) = server.request("GET", path, None, None);
            assert_eq!(status, 200, "{path}");
            assert!(mime.starts_with("text/javascript"), "{path}: {mime}");
        }
        let (status, font, mime) = server.request("GET", "/fonts/space-grotesk-700.ttf", None, None);
        assert_eq!(status, 200);
        assert_eq!(mime, "font/ttf");
        assert!(!font.is_empty());
        let (status, sample, _) = server.request("GET", "/samples/calgary.zql", None, None);
        assert_eq!(status, 200);
        assert!(String::from_utf8(sample).unwrap().contains("display"));
        let (_, body, _) = server.request("GET", "/explorer-config.json", None, None);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"backend":"native"})
        );
        for path in [
            "/.git/config",
            "/Cargo.toml",
            "/missing.js",
            "/pkg/missing.wasm",
        ] {
            assert_eq!(server.request("GET", path, None, None).0, 404);
        }
        assert_eq!(
            server.zql("mutation { Player(name: \"Explorer\" && salary: 9) { name salary } }"),
            json!({"name":"Explorer","salary":9})
        );
    }
    let server = Running::start("start", directory.path(), &[]);
    assert_eq!(
        server.zql("{ Player { name salary } }"),
        json!([{"name":"Explorer","salary":9}])
    );
}

#[test]
fn help_version_and_defaults_are_available_without_starting_a_server() {
    let version = Command::new(BIN).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("zega {}", env!("CARGO_PKG_VERSION"))
    );
    for args in [
        vec!["--help"],
        vec!["start", "--help"],
        vec!["explorer", "--help"],
    ] {
        let output = Command::new(BIN).args(&args).output().unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        if args[0] == "start" {
            assert!(
                help.contains("9342")
                    && help.contains("127.0.0.1")
                    && help.contains("--token-file")
                    && help.contains("--max-import-bytes")
            );
        }
        if args[0] == "explorer" {
            assert!(help.contains("9343"));
        }
    }
}

#[test]
fn only_one_cli_process_owns_a_data_directory() {
    let directory = tempfile::tempdir().unwrap();
    {
        let _server = Running::start("start", directory.path(), &[]);
        let output = Command::new(BIN)
            .args(["explorer", "--port", "0", "--data"])
            .arg(directory.path().join("db"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("already in use"));
    }
    let server = Running::start("explorer", directory.path(), &[]);
    assert_eq!(server.request("GET", "/health", None, None).0, 200);
}

// ---------------------------------------------------------------------------
// `zega export` / `zega import`: the .graph file on the command line.

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../zega/tests/fixtures/golden-v1.graph");

fn zega(args: &[&str], directory: &Path) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn succeeds(args: &[&str], directory: &Path) -> std::process::Output {
    let output = zega(args, directory);
    assert!(
        output.status.success(),
        "zega {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn import_then_export_round_trips_and_every_later_export_is_identical() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let output = succeeds(&["import", GOLDEN, "--data", "a"], dir);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        format!("imported 3 nodes and 2 relationships from {GOLDEN} (.graph format 1, written by zega 0.2.0)")
    );
    let output = succeeds(&["export", "a.graph", "--data", "a"], dir);
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("wrote 3 nodes and 2 relationships to a.graph"));
    assert!(!dir.join("a.graph.partial").exists());
    // Through a second store and back out: the same bytes.
    succeeds(&["import", "a.graph", "--data", "b"], dir);
    succeeds(&["export", "b.graph", "--data", "b"], dir);
    let a = std::fs::read(dir.join("a.graph")).unwrap();
    // Import then export is the same file, schema and metadata included.
    assert_eq!(a, std::fs::read(GOLDEN).unwrap());
    assert_eq!(std::fs::read(dir.join("b.graph")).unwrap(), a);
    // `-` is stdout.
    assert_eq!(succeeds(&["export", "-", "--data", "b"], dir).stdout, a);
    // A server on the imported store serves the same bytes.
    std::fs::rename(dir.join("b"), dir.join("db")).unwrap();
    let server = Running::start("start", dir, &[]);
    let (status, body, mime) = server.request("GET", "/graph", None, None);
    assert_eq!((status, mime.as_str()), (200, "application/vnd.zega.graph"));
    assert_eq!(body, a);
}

#[test]
fn export_carries_a_schema_and_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    std::fs::write(dir.join("schema.zql"), "type City { name: String }\nunique { City { name } }\n").unwrap();
    succeeds(&["import", GOLDEN, "--data", "a"], dir);
    succeeds(
        &["export", "a.graph", "--data", "a", "--schema", "schema.zql", "--meta", "licence=CC0-1.0", "--meta", "title=Cities"],
        dir,
    );
    let summary = zega::Zega::in_memory()
        .build()
        .unwrap()
        .import(std::fs::File::open(dir.join("a.graph")).unwrap())
        .unwrap();
    assert_eq!(summary.schema.as_deref(), Some("type City { name: String }\nunique { City { name } }\n"));
    assert_eq!(summary.uniques, vec![("City".to_string(), "name".to_string())]);
    assert_eq!(summary.meta["licence"], "CC0-1.0");
    assert_eq!(summary.meta["title"], "Cities");
    let output = zega(&["export", "b.graph", "--data", "a", "--meta", "no-equals"], dir);
    assert!(!output.status.success());
    assert!(!dir.join("b.graph").exists());
}

#[test]
fn import_refuses_to_overwrite_without_replace_and_a_damaged_file_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    {
        let server = Running::start("start", dir, &[]);
        server.zql("mutation { Player(name: \"Ada\" && salary: 1) { name } }");
    }
    let before = succeeds(&["export", "-", "--data", "db"], dir).stdout;

    let output = zega(&["import", GOLDEN, "--data", "db"], dir);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("pass --replace"), "{output:?}");

    let golden = std::fs::read(GOLDEN).unwrap();
    std::fs::write(dir.join("cut.graph"), &golden[..golden.len() / 2]).unwrap();
    let output = zega(&["import", "cut.graph", "--data", "db", "--replace"], dir);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("zega import: truncated .graph file"), "{output:?}");
    assert_eq!(succeeds(&["export", "-", "--data", "db"], dir).stdout, before);

    succeeds(&["import", GOLDEN, "--data", "db", "--replace"], dir);
    assert_ne!(succeeds(&["export", "-", "--data", "db"], dir).stdout, before);
}

#[test]
fn export_and_import_respect_a_running_server_s_lock() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let _server = Running::start("start", dir, &[]);
    for args in [["export", "x.graph", "--data", "db"], ["import", GOLDEN, "--data", "db"]] {
        let output = zega(&args, dir);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("already in use"), "{output:?}");
    }
    assert!(!dir.join("x.graph").exists());
}

/// The schema of the kill -9 test: a note big enough to grow the WAL fast.
const LOAD_SCHEMA: &str = "type Player { name: String salary: Int note: String }";

/// POST one ZQL statement; `None` when the server did not answer it (it was
/// killed), so the write was never acknowledged.
fn try_zql(url: &str, query: &str) -> Option<Value> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .new_agent();
    let body = serde_json::to_vec(&json!({"schema": LOAD_SCHEMA, "query": query})).unwrap();
    let request = ureq::http::Request::builder()
        .method("POST")
        .uri(format!("{url}/zql"))
        .header("Content-Type", "application/json")
        .body(body)
        .unwrap();
    let mut response = agent.run(request).ok()?;
    let body = response.body_mut().read_to_vec().ok()?;
    let body: Value = serde_json::from_slice(&body).ok()?;
    (response.status().as_u16() == 200 && body["ok"] == json!(true)).then(|| body["result"].clone())
}

/// What the writers of the kill -9 test were told, and what they tried.
#[derive(Default)]
struct Load {
    /// Nodes created and acknowledged / attempted.
    created: std::collections::BTreeSet<String>,
    tried: std::collections::BTreeSet<String>,
    /// Per writer: the last `salary` its counter node was acknowledged at,
    /// and the last one it tried.
    counter_acked: std::collections::BTreeMap<usize, i64>,
    counter_tried: std::collections::BTreeMap<usize, i64>,
}

/// zega#52: `kill -9` a real `zega start` while four writers keep it busy
/// and it checkpoints (every MiB here), six times over one data directory.
/// Half the writes create nodes, half rewrite one big node per writer, so
/// the WAL grows much faster than the graph and checkpoints come often.
/// After every restart each acknowledged write is there (a created node
/// exists; a counter is at least its last acknowledged value), nothing
/// that was never written is, and the server reopened.
#[test]
fn kill_9_under_write_load_loses_no_acknowledged_write_and_always_reopens() {
    use std::sync::{Arc, Mutex};

    const WRITERS: usize = 4;
    let directory = tempfile::tempdir().unwrap();
    let args = ["--snapshot-every-mb", "1"];
    let pad = "x".repeat(96 * 1024);
    let load = Arc::new(Mutex::new(Load::default()));
    {
        let server = Running::start("start", directory.path(), &args);
        for writer in 0..WRITERS {
            try_zql(&server.url, &format!(r#"mutation {{ Player(name: "c{writer}" && salary: 0 && note: "") {{ name }} }}"#))
                .expect("counter node");
        }
    }
    let mut killed_mid_checkpoint = 0;
    // Each round runs until its writers have had enough writes acknowledged
    // (however fast the machine), then a little longer, varied so the kill
    // lands at different points of a checkpoint.
    const PER_ROUND: usize = 30;
    for (round, extra) in [300u64, 900, 0, 1_500, 600, 1_100].into_iter().enumerate() {
        let mut server = Running::start("start", directory.path(), &args);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let (url, pad, stop, load) =
                    (server.url.clone(), pad.clone(), Arc::clone(&stop), Arc::clone(&load));
                thread::spawn(move || {
                    for i in 0.. {
                        if stop.load(std::sync::atomic::Ordering::SeqCst) {
                            return;
                        }
                        if i % 2 == 0 {
                            let name = format!("r{round}-w{writer}-{i}");
                            load.lock().unwrap().tried.insert(name.clone());
                            let query = format!(r#"mutation {{ Player(name: "{name}" && salary: {i} && note: "") {{ name }} }}"#);
                            if try_zql(&url, &query).is_none() {
                                return;
                            }
                            load.lock().unwrap().created.insert(name);
                        } else {
                            let value = (round as i64) * 1_000_000 + i;
                            load.lock().unwrap().counter_tried.insert(writer, value);
                            let query = format!(r#"mutation {{ Player(name: "c{writer}") set salary: {value}, note: "{pad}" }}"#);
                            if try_zql(&url, &query).is_none() {
                                return;
                            }
                            load.lock().unwrap().counter_acked.insert(writer, value);
                        }
                    }
                })
            })
            .collect();
        let target = PER_ROUND * (round + 1);
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        while load.lock().unwrap().created.len() < target {
            assert!(
                std::time::Instant::now() < deadline,
                "round {round}: {} of {target} creates acknowledged in 180 s",
                load.lock().unwrap().created.len()
            );
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(extra));
        server.child.kill().unwrap(); // SIGKILL
        server.child.wait().unwrap();
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        for writer in writers {
            writer.join().unwrap();
        }
        drop(server);
        let data = directory.path().join("db");
        let mid_checkpoint = data.join("wal.rotate.tmp").exists()
            || std::fs::read_dir(data.join("graphs")).is_ok_and(|mut dir| {
                dir.any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".partial"))
            });
        killed_mid_checkpoint += usize::from(mid_checkpoint);

        let server = Running::start("start", directory.path(), &args);
        let rows = server.zql("{ Player { name salary } }");
        let rows = rows.as_array().unwrap();
        let load = load.lock().unwrap();
        let present: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_string())
            .filter(|name| !name.starts_with('c'))
            .collect();
        let lost: Vec<_> = load.created.difference(&present).collect();
        assert!(lost.is_empty(), "round {round}: {} acknowledged creates lost: {lost:?}", lost.len());
        assert!(present.is_subset(&load.tried), "round {round}: a node nobody created");
        for writer in 0..WRITERS {
            let name = format!("c{writer}");
            let counters: Vec<i64> = rows
                .iter()
                .filter(|row| row["name"] == json!(name))
                .map(|row| row["salary"].as_i64().unwrap())
                .collect();
            let acked = load.counter_acked.get(&writer).copied().unwrap_or(0);
            let tried = load.counter_tried.get(&writer).copied().unwrap_or(0);
            assert!(
                counters.len() == 1 && (acked..=tried).contains(&counters[0]),
                "round {round}: counter {writer} is {counters:?}, acknowledged at {acked}, tried up to {tried}"
            );
        }
        assert!(load.created.len() >= PER_ROUND * (round + 1), "round {round}: too little load to test anything");
        println!(
            "round {round}: killed {extra} ms after {} creates{}; {} creates acknowledged so far; wal.bin {} bytes",
            PER_ROUND * (round + 1),
            if mid_checkpoint { " mid-checkpoint" } else { "" },
            load.created.len(),
            std::fs::metadata(data.join("wal.bin")).unwrap().len(),
        );
    }
    println!("{killed_mid_checkpoint} of 6 kills landed mid-checkpoint");
    // Checkpoints happened: the WAL starts from one (its first entry is a
    // `ReplaceGraph`, variant 6) and is bounded, not every write.
    let wal = std::fs::read(directory.path().join("db").join("wal.bin")).unwrap();
    assert_eq!(wal[18..22], 6u32.to_le_bytes(), "the WAL does not start from a checkpoint");
}
