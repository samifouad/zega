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

/// zega#52: `docker stop` sends SIGTERM. The server stops cleanly and
/// checkpoints, so the next start reads a snapshot and a log that holds
/// nothing but the checkpoint entry.
#[cfg(unix)]
#[test]
fn sigterm_checkpoints_and_the_next_start_reads_the_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = Running::start("start", directory.path(), &[]);
    for (name, salary) in [("Ada", 42), ("Grace", 7)] {
        server.zql(&format!(
            "mutation {{ Player(name: \"{name}\" && salary: {salary}) {{ name }} }}"
        ));
    }
    let wal = directory.path().join("db/wal.bin");
    let before = std::fs::metadata(&wal).unwrap().len();
    assert!(!directory.path().join("db/snapshot.bin").exists());
    let killed = Command::new("kill")
        .args(["-TERM", &server.child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let mut status = None;
    for _ in 0..300 {
        status = server.child.try_wait().unwrap();
        if status.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let status = status.expect("the server was still running 30 s after SIGTERM");
    assert!(status.success(), "SIGTERM should stop the server cleanly: {status}");
    assert!(directory.path().join("db/snapshot.bin").exists());
    let after = std::fs::metadata(&wal).unwrap().len();
    assert!(after < 64, "log is {after} bytes after shutdown (was {before})");
    drop(server);

    let server = Running::start("start", directory.path(), &[]);
    let mut players = server.zql("{ Player { name salary } }");
    players
        .as_array_mut()
        .unwrap()
        .sort_by_key(|player| player["name"].to_string());
    assert_eq!(
        players,
        json!([{"name":"Ada","salary":42},{"name":"Grace","salary":7}])
    );
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
                    && help.contains("--checkpoint-mb")
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
