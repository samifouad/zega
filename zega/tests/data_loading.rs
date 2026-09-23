use serde_json::{json, Value};
use std::collections::HashMap;
#[cfg(feature = "http")]
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use zega::Zega;

const SCHEMA: &str =
    "type Player { name: String salary: Int } type Team { name: String players -> Player[] }";
const QUERY: &str = "{ Team { name players -> Player { name salary } } }";
const EXPECTED: &str = "Ada, A.";
const JSON: &str = r#"[{"Name":"Ada, A.","Salary":42,"Team":"A"}]"#;
const CSV: &str = "Name,Salary,Team\r\n\"Ada, A.\",42,A\r\n";

fn mutation(format: &str, location: &str) -> String {
    format!("mutation {format} [{}] {{ Team(name: $Team) {{ name players -> Player(name: $Name && salary: $Salary) {{ name salary }} }} }}", serde_json::to_string(location).unwrap())
}

fn assert_loaded(db: &Zega) {
    assert_eq!(
        db.run_lang(SCHEMA, QUERY).unwrap(),
        json!([{"name":"A","players":[{"name":EXPECTED,"salary":42}]}])
    );
}

fn local_load(format: &str, text: &str) {
    // A relative location proves resolution against cwd, independently of DB path.
    let fixtures = tempfile::Builder::new()
        .prefix(".load-test-")
        .tempdir_in(".")
        .unwrap();
    let file = fixtures.path().join(format!("players.{format}"));
    std::fs::write(&file, text).unwrap();
    let memory = Zega::in_memory().build().unwrap();
    memory
        .run_lang(SCHEMA, &mutation(format, file.to_str().unwrap()))
        .unwrap();
    assert_loaded(&memory);
    let data = tempfile::tempdir().unwrap();
    {
        let db = Zega::open(data.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        db.run_lang(SCHEMA, &mutation(format, file.to_str().unwrap()))
            .unwrap();
        assert_loaded(&db);
    }
    // Remove the input: reopen must read the WAL, not re-import.
    std::fs::remove_file(file).unwrap();
    assert_loaded(&Zega::open(data.path().to_str().unwrap()).build().unwrap());
}

#[test]
fn local_json_memory_disk_and_wal_reopen() {
    local_load("json", JSON);
}
#[test]
fn local_csv_memory_disk_and_wal_reopen() {
    local_load("csv", CSV);
}

#[cfg(feature = "http")]
struct HttpFixture {
    url: String,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
#[cfg(feature = "http")]
impl HttpFixture {
    fn new(body: &str, status: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let task = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_nonblocking(false)
                        .expect("accepted HTTP fixture sockets must block while reading requests");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    // TCP reads may end anywhere in the headers. Closing with
                    // unread request bytes can reset the client's connection.
                    let mut request = BufReader::new(&mut stream);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        assert_ne!(request.read_line(&mut line).unwrap(), 0);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    stream.write_all(response.as_bytes()).unwrap();
                    stream.shutdown(Shutdown::Write).unwrap();
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        });
        Self {
            url,
            stop,
            task: Some(task),
        }
    }
}
#[cfg(feature = "http")]
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.take().unwrap().join().unwrap();
    }
}

#[cfg(feature = "http")]
#[test]
fn http_fixture_consumes_fragmented_request_headers() {
    let fixture = HttpFixture::new(CSV, "200 OK");
    let mut client = TcpStream::connect(fixture.url.strip_prefix("http://").unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    // Exceed the old single-read buffer and split the request across writes.
    let request = format!(
        "GET /players.csv HTTP/1.1\r\nHost: localhost\r\nX-Padding: {}\r\nConnection: close\r\n\r\n",
        "x".repeat(16 * 1024)
    );
    for chunk in request.as_bytes().chunks(17) {
        client.write_all(chunk).unwrap();
    }
    client.shutdown(Shutdown::Write).unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert_eq!(
        response,
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{CSV}",
            CSV.len()
        )
    );
}

#[cfg(feature = "http")]
fn remote_load(format: &str, text: &str) {
    let fixture = HttpFixture::new(text, "200 OK");
    let query = mutation(format, &format!("{}/players.{format}", fixture.url));
    let data = tempfile::tempdir().unwrap();
    let memory = Zega::in_memory()
        .allow_private_imports(true)
        .build()
        .unwrap();
    memory.run_lang(SCHEMA, &query).unwrap();
    assert_loaded(&memory);
    {
        let db = Zega::open(data.path().to_str().unwrap())
            .allow_private_imports(true)
            .wal_flush_every_write()
            .build()
            .unwrap();
        db.run_lang(SCHEMA, &query).unwrap();
        assert_loaded(&db);
    }
    drop(fixture);
    assert_loaded(&Zega::open(data.path().to_str().unwrap()).build().unwrap());
}

#[cfg(feature = "http")]
#[test]
fn remote_json_memory_disk_and_wal_reopen() {
    remote_load("json", JSON);
}
#[cfg(feature = "http")]
#[test]
fn remote_csv_memory_disk_and_wal_reopen() {
    remote_load("csv", CSV);
}

#[test]
fn bad_csv_is_rejected_before_any_rows_are_inserted() {
    let db = Zega::in_memory().build().unwrap();
    let text = "Name,Salary,Team\nValid,42,A\nBroken,12,A,EXTRA\n";
    let sources = HashMap::from([("./import".into(), text.into())]);
    let error = db
        .run_lang_with_sources(SCHEMA, &mutation("csv", "./import"), &sources)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("csv") && error.contains("record 2"),
        "{error}"
    );
    assert_eq!(
        db.run_lang(SCHEMA, "{ Player { name } }").unwrap(),
        json!([])
    );
}

#[test]
fn malformed_json_missing_file_and_http_error_are_readable() {
    let db = Zega::in_memory()
        .allow_private_imports(true)
        .build()
        .unwrap();
    let sources = HashMap::from([("./broken.json".into(), "[{bad]".into())]);
    let error = db
        .run_lang_with_sources(SCHEMA, &mutation("json", "./broken.json"), &sources)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("./broken.json is not json") && error.contains("line 1"),
        "{error}"
    );
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    let error = db
        .run_lang(SCHEMA, &mutation("json", missing.to_str().unwrap()))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("cannot read") && error.contains("missing.json"),
        "{error}"
    );
    #[cfg(feature = "http")]
    {
        let fixture = HttpFixture::new("absent", "404 Not Found");
        let error = db
            .run_lang(SCHEMA, &mutation("json", &fixture.url))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot read") && error.contains("404"),
            "{error}"
        );
    }
}

#[test]
fn supplied_sources_use_the_same_parser_and_writes() {
    let db = Zega::in_memory().build().unwrap();
    let sources = HashMap::from([("./import".into(), JSON.into())]);
    db.run_lang_with_sources(SCHEMA, &mutation("json", "./import"), &sources)
        .unwrap();
    assert_loaded(&db);
    let error = db
        .run_lang_with_sources(SCHEMA, &mutation("json", "missing"), &sources)
        .unwrap_err()
        .to_string();
    assert!(error.contains("host did not supply"), "{error}");
}

#[test]
fn oversized_source_rejected_before_insertion() {
    let db = Zega::in_memory().build().unwrap();
    let sources = HashMap::from([("./import".into(), " ".repeat(2_000_001))]);
    let error = db
        .run_lang_with_sources(SCHEMA, &mutation("json", "./import"), &sources)
        .unwrap_err()
        .to_string();
    assert!(error.contains("larger than 2MB"), "{error}");
    assert_eq!(
        db.run_lang(SCHEMA, "{ Player { name } }").unwrap(),
        Value::Array(vec![])
    );
}

#[cfg(not(feature = "http"))]
#[test]
fn disabled_http_feature_has_actionable_error() {
    let db = Zega::in_memory().build().unwrap();
    let error = db
        .run_lang(SCHEMA, &mutation("json", "https://example.com/data.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("`http` cargo feature"), "{error}");
}

#[test]
fn loaded_rows_link_existing_nodes_and_survive_reopen() {
    let schema = "type Team { name: String } type Player { name: String team -> Team }";
    let data = tempfile::tempdir().unwrap();
    let query = "{ Player { name team -> Team { name } } }";
    {
        let db = Zega::open(data.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        db.run_lang(schema, "mutation { Team(name: \"A\") { name } }")
            .unwrap();
        let sources = HashMap::from([("./import".into(), JSON.into())]);
        db.run_lang_with_sources(schema, "mutation json [\"./import\"] { Player(name: $Name) { name } }", &sources).unwrap();
        db.run_lang_with_sources(schema, "mutation json [\"./import\"] { Player(name: $Name) { name team -> link Team(name: $Team) { name } } }", &sources).unwrap();
        assert_eq!(
            db.run_lang(schema, "{ Team { name } }").unwrap(),
            json!([{"name":"A"}])
        );
    }
    let db = Zega::open(data.path().to_str().unwrap()).build().unwrap();
    assert_eq!(
        db.run_lang(schema, query).unwrap(),
        json!([{"name":EXPECTED,"team":{"name":"A"}}])
    );
}
