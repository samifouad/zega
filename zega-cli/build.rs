use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

fn copy_tree(source: &Path, destination: &Path) {
    println!("cargo:rerun-if-changed={}", source.display());
    if source.is_dir() {
        fs::create_dir_all(destination).expect("create explorer bundle directory");
        for entry in fs::read_dir(source).expect("read explorer assets") {
            let entry = entry.expect("read explorer asset");
            copy_tree(&entry.path(), &destination.join(entry.file_name()));
        }
    } else {
        fs::copy(source, destination).expect("copy explorer asset");
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let browser = root.join("browser");
    println!(
        "cargo:rerun-if-changed={}",
        browser.join("wasm-source.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        browser.join("assets.json").display()
    );
    let provenance: serde_json::Value =
        serde_json::from_slice(&fs::read(browser.join("wasm-source.json")).unwrap()).unwrap();
    for (name, expected) in provenance["sha256"]
        .as_object()
        .expect("wasm hash inventory")
    {
        let bytes = fs::read(browser.join("pkg").join(name)).expect("vendored wasm asset");
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            expected.as_str().unwrap(),
            "wasm hash mismatch for {name}; rebuild browser/pkg"
        );
    }
    let assets: Vec<String> =
        serde_json::from_slice(&fs::read(browser.join("assets.json")).unwrap()).unwrap();
    let output = std::path::PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("explorer");
    if output.exists() {
        fs::remove_dir_all(&output).unwrap();
    }
    fs::create_dir_all(&output).unwrap();
    for asset in assets {
        copy_tree(&browser.join(&asset), &output.join(&asset));
    }
    copy_tree(&root.join("LICENSE"), &output.join("LICENSE"));
}
