use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// Returns whether every input was already formatted in check mode.
pub fn run(
    paths: Vec<PathBuf>,
    check: bool,
    stdin: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    if stdin {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source)?;
        let formatted = zega::fmt::format_zql(&source)?;
        if check {
            if formatted != source {
                eprintln!("stdin would be reformatted");
            }
            return Ok(formatted == source);
        }
        io::stdout().write_all(formatted.as_bytes())?;
        return Ok(true);
    }
    let mut files = Vec::new();
    for path in paths {
        collect(&path, &mut files)?;
    }
    files.sort();
    files.dedup();
    let mut clean = true;
    for path in &files {
        let source = fs::read_to_string(path)?;
        let formatted = zega::fmt::format_zql(&source)?;
        if formatted == source {
            continue;
        }
        clean = false;
        if check {
            println!("{} would be reformatted", path.display());
        } else {
            fs::write(path, formatted)?;
            println!("formatted {}", path.display());
        }
    }
    eprintln!("checked {} ZQL file(s)", files.len());
    Ok(!check || clean)
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    // Never follow a directory link into another checkout (or a cycle).
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        out.push(path.to_owned());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.')
                || matches!(name.to_str(), Some("node_modules" | "target" | "dist"))
            {
                continue;
            }
            collect(&entry.path(), out)?;
        } else if entry.path().extension().is_some_and(|ext| ext == "zql") {
            out.push(entry.path());
        }
    }
    Ok(())
}
