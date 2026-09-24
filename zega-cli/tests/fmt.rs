use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};
const BIN: &str = env!("CARGO_BIN_EXE_zega");
const INPUT: &str = "query{Player{name salary}}";
const OUTPUT: &str = "query {\n  Player { name salary }\n}\n";
#[test]
fn stdin_and_check_exit_codes() {
    for (source, args, status, stdout) in [
        (INPUT, vec!["fmt", "--stdin"], 0, OUTPUT),
        (INPUT, vec!["fmt", "--stdin", "--check"], 1, ""),
        (OUTPUT, vec!["fmt", "--stdin", "--check"], 0, ""),
        (
            r#"{"x":1e3,"y":0.10}"#,
            vec!["fmt", "--stdin", "--lang", "json"],
            0,
            "{ \"x\": 1e3, \"y\": 0.10 }\n",
        ),
        ("query{", vec!["fmt", "--stdin"], 0, "query{"),
    ] {
        let mut child = Command::new(BIN)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(status));
        assert_eq!(String::from_utf8(output.stdout).unwrap(), stdout);
    }
}
#[test]
fn recursive_check_lists_every_change_and_rewrite_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    for file in ["one.zql", "nested/two.zql", "ignore.txt"] {
        fs::write(dir.path().join(file), INPUT).unwrap();
    }
    fs::write(dir.path().join("data.json"), r#"{"a":1e3,"b":0.10}"#).unwrap();
    let check = || {
        Command::new(BIN)
            .args(["fmt", "--check"])
            .arg(dir.path())
            .output()
            .unwrap()
    };
    let before = check();
    assert_eq!(before.status.code(), Some(1));
    let stdout = String::from_utf8(before.stdout).unwrap();
    assert!(stdout.contains("one.zql") && stdout.contains("two.zql"));
    assert!(!stdout.contains("ignore.txt"));
    assert!(stdout.contains("data.json"));
    assert_eq!(
        fs::read_to_string(dir.path().join("one.zql")).unwrap(),
        INPUT
    );
    assert!(Command::new(BIN)
        .arg("fmt")
        .arg(dir.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(check().status.success());
    assert_eq!(
        fs::read_to_string(dir.path().join("data.json")).unwrap(),
        "{ \"a\": 1e3, \"b\": 0.10 }\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("nested/two.zql")).unwrap(),
        OUTPUT
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("ignore.txt")).unwrap(),
        INPUT
    );
}
#[test]
fn bad_paths_and_conflicting_arguments_fail() {
    for args in [
        vec!["fmt"],
        vec!["fmt", "--stdin", "file.zql"],
        vec!["fmt", "/does-not-exist-fmt.zql"],
    ] {
        assert!(!Command::new(BIN)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let help = Command::new(BIN).args(["fmt", "--help"]).output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--stdin") && help.contains("--check"));
}
