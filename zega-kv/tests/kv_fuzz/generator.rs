use zega_parser::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Set {
        key: usize,
        value: Value,
        ttl: Option<u64>,
    },
    Get {
        key: usize,
    },
    Del {
        key: usize,
    },
    Incr {
        key: usize,
    },
    Lpush {
        key: usize,
        value: Value,
    },
    Lrange {
        key: usize,
        start: usize,
        stop: usize,
    },
    Ltrim {
        key: usize,
        start: usize,
        stop: usize,
    },
}

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn usize(&mut self, upper: usize) -> usize {
        self.next() as usize % upper
    }
}

pub fn sequence(rng: &mut Rng) -> Vec<Op> {
    (0..8 + rng.usize(25)).map(|_| op(rng)).collect()
}

fn op(rng: &mut Rng) -> Op {
    let key = rng.usize(6);
    match rng.usize(10) {
        0 | 1 => Op::Set {
            key,
            value: value(rng),
            ttl: [None, Some(0), Some(3600), Some(86_400)][rng.usize(4)],
        },
        2 => Op::Get { key },
        3 => Op::Del { key },
        4 | 5 => Op::Incr { key },
        6 | 7 => Op::Lpush {
            key,
            value: value(rng),
        },
        8 => {
            let start = rng.usize(5);
            Op::Lrange {
                key,
                start,
                stop: start + rng.usize(7),
            }
        }
        _ => {
            let start = rng.usize(3);
            Op::Ltrim {
                key,
                start,
                stop: start + 1 + rng.usize(5),
            }
        }
    }
}

fn value(rng: &mut Rng) -> Value {
    match rng.usize(8) {
        0 => Value::Int(-1),
        1 => Value::Int(-9_000_000_000_000_000_000),
        2 => Value::Int(0),
        3 => Value::Int(9_000_000_000_000_000_000),
        4 => Value::String(String::new()),
        5 => Value::String("hello".into()),
        6 => Value::String("unicode-\u{96ea}-\u{1f680}".into()),
        _ => Value::String(format!("large-{}", "x".repeat(4096))),
    }
}
