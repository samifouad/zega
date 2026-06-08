use std::process::Command;

#[test]
fn kv_generative_differential_fuzzer() {
    let target = format!(
        "{}/../target/kv-fuzz-nested-{}",
        env!("CARGO_MANIFEST_DIR"),
        std::process::id()
    );
    std::fs::create_dir_all(&target).expect("create isolated nested-cargo target");
    let status = Command::new(env!("CARGO"))
        .args([
            "test",
            "-p",
            "zega-kv",
            "--test",
            "kv_fuzz",
            "--",
            "--nocapture",
        ])
        .env("CARGO_TARGET_DIR", target)
        .status()
        .expect("run zega-kv differential fuzzer");
    assert!(status.success(), "zega-kv differential fuzzer failed");
}
