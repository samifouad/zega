//! Generative differential parity for Zega's curated Redis-compatible KV surface.
//!
//! `EXISTS`, `EXPIRE`, `TTL`, and arbitrary-delta `INCRBY` are not public
//! `KvStore` operations, so they are intentionally not generated. Zega stores
//! typed values and uses exclusive unsigned list stops; the Redis adapter
//! restores types and translates list stops. Redis deletes lists trimmed to
//! empty while Zega retains them, so the Redis adapter uses a filtered private
//! sentinel to preserve empty-list existence. TTL zero is made deterministic
//! by deleting the Redis value immediately; positive TTLs are long-lived
//! because neither backend exposes a controllable clock.

#[path = "kv_fuzz/generator.rs"]
mod generator;

use generator::{Op, Rng};
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};
use zega_kv::KvStore;
use zega_parser::Value;

const DEFAULT_SEED: u64 = 0x6b76_5f66_757a_7a21;

#[derive(Debug, PartialEq, Eq)]
enum Obs {
    Unit,
    Bool(bool),
    Value(Option<Value>),
}

#[test]
fn generator_is_seeded_and_reproducible() {
    let mut left = Rng::new(42);
    let mut right = Rng::new(42);
    for _ in 0..20 {
        assert_eq!(
            generator::sequence(&mut left),
            generator::sequence(&mut right)
        );
    }
}

#[test]
fn sequence_shrinker_finds_one_operation_repro() {
    let ops = generator::sequence(&mut Rng::new(7));
    let shrunk = shrink_with(ops, |candidate| {
        candidate.iter().any(|op| matches!(op, Op::Incr { .. }))
    });
    assert_eq!(shrunk.len(), 1);
    assert!(matches!(shrunk[0], Op::Incr { .. }));
}

#[test]
fn generated_grammar_covers_supported_surface_and_edges() {
    let mut rng = Rng::new(DEFAULT_SEED);
    let mut seen = [false; 7];
    let mut zero_ttl = false;
    let mut unicode = false;
    let mut large = false;
    let mut negative = false;
    for _ in 0..1000 {
        for op in generator::sequence(&mut rng) {
            let value = match &op {
                Op::Set { value, ttl, .. } => {
                    zero_ttl |= *ttl == Some(0);
                    Some(value)
                }
                Op::Lpush { value, .. } => Some(value),
                _ => None,
            };
            if let Some(Value::String(value)) = value {
                unicode |= value.contains('\u{96ea}');
                large |= value.len() > 4000;
            }
            negative |= matches!(value, Some(Value::Int(n)) if *n < 0);
            seen[match op {
                Op::Set { .. } => 0,
                Op::Get { .. } => 1,
                Op::Del { .. } => 2,
                Op::Incr { .. } => 3,
                Op::Lpush { .. } => 4,
                Op::Lrange { .. } => 5,
                Op::Ltrim { .. } => 6,
            }] = true;
        }
    }
    assert!(seen.into_iter().all(|covered| covered));
    assert!(zero_ttl && unicode && large && negative);
}

#[test]
fn generative_kv_differential_parity() {
    let seed = env_u64("ZEGA_PARITY_SEED", DEFAULT_SEED);
    let cases = env_u64("ZEGA_PARITY_CASES", 1000) as usize;
    if env::var("ZEGA_PARITY_LIVE").as_deref() != Ok("1") {
        println!("KV GENERATIVE PARITY: 0/0 cases; live compare gated by ZEGA_PARITY_LIVE=1");
        return;
    }

    let mut redis = Redis::connect();
    let run = format!(
        "zega-kv-fuzz:{}:{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut rng = Rng::new(seed);
    let mut mismatches = Vec::new();
    for case in 0..cases {
        let ops = generator::sequence(&mut rng);
        if run_sequence(&mut redis, &run, case, &ops).is_err() {
            let minimal = shrink_live(&mut redis, &run, case, ops);
            let minimal_diff = run_sequence(&mut redis, &run, case, &minimal).unwrap_err();
            println!("\nMINIMAL KV REPRO seed={seed} case={case}\n{minimal:#?}\n{minimal_diff}");
            mismatches.push(case);
        }
    }
    let passed = cases - mismatches.len();
    println!(
        "KV GENERATIVE PARITY: {passed}/{cases} cases, {} mismatches (seed={seed})",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "KV parity mismatches in cases {mismatches:?}"
    );
}

fn run_sequence(redis: &mut Redis, run: &str, case: usize, ops: &[Op]) -> Result<(), String> {
    let keys: Vec<_> = (0..6).map(|key| format!("{run}:{case}:{key}")).collect();
    redis.del_all(&keys);
    let zega = KvStore::new();
    for (index, op) in ops.iter().enumerate() {
        let key = &keys[op_key(op)];
        let left = zega_op(&zega, key, op);
        let right = redis.op(key, op);
        if left != right {
            redis.del_all(&keys);
            return Err(format!(
                "op {index} {op:?}\n  zega: {left:?}\n  redis: {right:?}"
            ));
        }
        let left_read = zega.get(key);
        let right_read = redis.read_back(key);
        if left_read != right_read {
            redis.del_all(&keys);
            return Err(format!(
                "read-back after op {index} {op:?}\n  zega: {left_read:?}\n  redis: {right_read:?}"
            ));
        }
    }
    redis.del_all(&keys);
    Ok(())
}

fn zega_op(zega: &KvStore, key: &str, op: &Op) -> Obs {
    match op {
        Op::Set { value, ttl, .. } => {
            zega.set(key.into(), value.clone(), *ttl);
            Obs::Unit
        }
        Op::Get { .. } => Obs::Value(zega.get(key)),
        Op::Del { .. } => Obs::Bool(zega.del(key)),
        Op::Incr { .. } => Obs::Value(zega.incr(key)),
        Op::Lpush { value, .. } => {
            zega.lpush(key, value.clone());
            Obs::Unit
        }
        Op::Lrange { start, stop, .. } => {
            Obs::Value(zega.lrange(key, *start, *stop).unwrap().map(Value::List))
        }
        Op::Ltrim { start, stop, .. } => Obs::Bool(zega.ltrim(key, *start, *stop).unwrap()),
    }
}

fn op_key(op: &Op) -> usize {
    match op {
        Op::Set { key, .. }
        | Op::Get { key }
        | Op::Del { key }
        | Op::Incr { key }
        | Op::Lpush { key, .. }
        | Op::Lrange { key, .. }
        | Op::Ltrim { key, .. } => *key,
    }
}

fn shrink_live(redis: &mut Redis, run: &str, case: usize, ops: Vec<Op>) -> Vec<Op> {
    shrink_with(ops, |candidate| {
        run_sequence(redis, run, case, candidate).is_err()
    })
}

fn shrink_with(mut ops: Vec<Op>, mut mismatch: impl FnMut(&[Op]) -> bool) -> Vec<Op> {
    loop {
        let mut changed = false;
        for index in 0..ops.len() {
            let mut candidate = ops.clone();
            candidate.remove(index);
            if !candidate.is_empty() && mismatch(&candidate) {
                ops = candidate;
                changed = true;
                break;
            }
        }
        if !changed {
            return ops;
        }
    }
}

struct Redis {
    reader: BufReader<TcpStream>,
}

const EMPTY_LIST: &[u8] = b"__zega_kv_fuzz_empty_list__";

#[derive(Debug)]
enum Resp {
    Simple(String),
    Error,
    Int(i64),
    Bulk(Option<Vec<u8>>),
    Array(Vec<Resp>),
}

impl Redis {
    fn connect() -> Self {
        let url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
        let address = url
            .strip_prefix("redis://")
            .unwrap_or(&url)
            .trim_end_matches('/');
        let stream = TcpStream::connect(address)
            .unwrap_or_else(|error| panic!("Redis reference connection failed ({url}): {error}"));
        stream.set_nodelay(true).expect("enable Redis TCP_NODELAY");
        Self {
            reader: BufReader::new(stream),
        }
    }

    fn op(&mut self, key: &str, op: &Op) -> Obs {
        match op {
            Op::Set { value, ttl, .. } => {
                let value = wire(value);
                if *ttl == Some(0) {
                    self.cmd(&[b"SET", key.as_bytes(), &value]);
                    self.cmd(&[b"DEL", key.as_bytes()]);
                } else if let Some(ttl) = ttl {
                    let ttl = ttl.to_string();
                    self.cmd(&[b"SET", key.as_bytes(), &value, b"EX", ttl.as_bytes()]);
                } else {
                    self.cmd(&[b"SET", key.as_bytes(), &value]);
                }
                Obs::Unit
            }
            Op::Get { .. } => Obs::Value(self.read_back(key)),
            Op::Del { .. } => Obs::Bool(self.int(&[b"DEL", key.as_bytes()]) == 1),
            Op::Incr { .. } => match self.cmd(&[b"INCR", key.as_bytes()]) {
                Resp::Int(value) => Obs::Value(Some(Value::Int(value))),
                Resp::Error => Obs::Value(None),
                other => panic!("unexpected INCR response: {other:?}"),
            },
            Op::Lpush { value, .. } => {
                if self.kind(key) == "string" || self.is_empty_list(key) {
                    self.cmd(&[b"DEL", key.as_bytes()]);
                }
                let value = wire(value);
                self.cmd(&[b"LPUSH", key.as_bytes(), &value]);
                Obs::Unit
            }
            Op::Lrange { start, stop, .. } => Obs::Value(self.list_range(key, *start, *stop)),
            Op::Ltrim { start, stop, .. } => {
                if self.kind(key) != "list" {
                    return Obs::Bool(false);
                }
                if self.is_empty_list(key) {
                    return Obs::Bool(true);
                }
                if start >= stop {
                    self.cmd(&[b"DEL", key.as_bytes()]);
                } else {
                    let start = start.to_string();
                    let stop = (stop - 1).to_string();
                    self.cmd(&[b"LTRIM", key.as_bytes(), start.as_bytes(), stop.as_bytes()]);
                }
                if self.kind(key) == "none" {
                    self.cmd(&[b"LPUSH", key.as_bytes(), EMPTY_LIST]);
                }
                Obs::Bool(true)
            }
        }
    }

    fn read_back(&mut self, key: &str) -> Option<Value> {
        match self.kind(key).as_str() {
            "none" => None,
            "string" => self.scalar(key),
            "list" => self.list_range(key, 0, usize::MAX),
            kind => panic!("unexpected Redis type {kind}"),
        }
    }

    fn scalar(&mut self, key: &str) -> Option<Value> {
        match self.cmd(&[b"GET", key.as_bytes()]) {
            Resp::Bulk(value) => value.map(|value| unwire(&value)),
            Resp::Error => None,
            other => panic!("unexpected GET response: {other:?}"),
        }
    }

    fn list_range(&mut self, key: &str, start: usize, stop: usize) -> Option<Value> {
        match self.kind(key).as_str() {
            "none" => None,
            "string" => None,
            "list" => {
                if start >= stop {
                    return Some(Value::List(Vec::new()));
                }
                let start = start.to_string();
                let stop = if stop == usize::MAX {
                    "-1".into()
                } else {
                    (stop - 1).to_string()
                };
                match self.cmd(&[b"LRANGE", key.as_bytes(), start.as_bytes(), stop.as_bytes()]) {
                    Resp::Array(values) => Some(Value::List(
                        values
                            .into_iter()
                            .filter_map(|value| match value {
                                Resp::Bulk(Some(value)) if value == EMPTY_LIST => None,
                                Resp::Bulk(Some(value)) => Some(unwire(&value)),
                                other => panic!("unexpected LRANGE item: {other:?}"),
                            })
                            .collect(),
                    )),
                    other => panic!("unexpected LRANGE response: {other:?}"),
                }
            }
            kind => panic!("unexpected Redis type {kind}"),
        }
    }

    fn is_empty_list(&mut self, key: &str) -> bool {
        matches!(self.list_range(key, 0, usize::MAX), Some(Value::List(values)) if values.is_empty())
    }

    fn kind(&mut self, key: &str) -> String {
        match self.cmd(&[b"TYPE", key.as_bytes()]) {
            Resp::Simple(kind) => kind,
            other => panic!("unexpected TYPE response: {other:?}"),
        }
    }

    fn del_all(&mut self, keys: &[String]) {
        let mut args: Vec<&[u8]> = vec![b"DEL"];
        args.extend(keys.iter().map(|key| key.as_bytes()));
        self.cmd(&args);
    }

    fn int(&mut self, args: &[&[u8]]) -> i64 {
        match self.cmd(args) {
            Resp::Int(value) => value,
            other => panic!("expected integer: {other:?}"),
        }
    }

    fn cmd(&mut self, args: &[&[u8]]) -> Resp {
        let stream = self.reader.get_mut();
        write!(stream, "*{}\r\n", args.len()).unwrap();
        for arg in args {
            write!(stream, "${}\r\n", arg.len()).unwrap();
            stream.write_all(arg).unwrap();
            stream.write_all(b"\r\n").unwrap();
        }
        stream.flush().unwrap();
        read_resp(&mut self.reader)
    }
}

fn read_resp(reader: &mut impl BufRead) -> Resp {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).unwrap();
    let prefix = line[0];
    let body = &line[1..line.len() - 2];
    match prefix {
        b'+' => Resp::Simple(String::from_utf8(body.to_vec()).unwrap()),
        b'-' => Resp::Error,
        b':' => Resp::Int(std::str::from_utf8(body).unwrap().parse().unwrap()),
        b'$' => {
            let len: isize = std::str::from_utf8(body).unwrap().parse().unwrap();
            if len < 0 {
                Resp::Bulk(None)
            } else {
                let mut value = vec![0; len as usize + 2];
                reader.read_exact(&mut value).unwrap();
                value.truncate(len as usize);
                Resp::Bulk(Some(value))
            }
        }
        b'*' => {
            let len: usize = std::str::from_utf8(body).unwrap().parse().unwrap();
            Resp::Array((0..len).map(|_| read_resp(reader)).collect())
        }
        _ => panic!("unknown RESP prefix {prefix}"),
    }
}

fn wire(value: &Value) -> Vec<u8> {
    match value {
        Value::Int(value) => value.to_string().into_bytes(),
        Value::String(value) => format!("s:{value}").into_bytes(),
        _ => unreachable!("generator only emits integers and strings"),
    }
}

fn unwire(value: &[u8]) -> Value {
    if value.starts_with(b"s:") {
        Value::String(String::from_utf8(value[2..].to_vec()).unwrap())
    } else {
        Value::Int(std::str::from_utf8(value).unwrap().parse().unwrap())
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{name} must be an unsigned integer"))
        })
        .unwrap_or(default)
}
