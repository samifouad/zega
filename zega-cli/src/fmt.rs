use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Language {
    Zql,
    Json,
}
impl Language {
    fn format(self, source: &str) -> Result<String, String> {
        match self {
            Self::Zql => zega::fmt::format_zql(source),
            Self::Json => Ok(zega::fmt::format_json(source)),
        }
    }
}

/// Returns whether every input was already formatted in check mode. An error
/// names the file or directory it happened on, so the caller can report it and
/// exit with a code of its own rather than the one `--check` uses (zegadb/zega#66).
pub fn run(
    paths: Vec<PathBuf>,
    check: bool,
    stdin: bool,
    lang: Option<Language>,
) -> Result<bool, String> {
    if stdin {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("stdin: {error}"))?;
        let formatted = lang
            .unwrap_or(Language::Zql)
            .format(&source)
            .map_err(|error| format!("stdin: {error}"))?;
        if check {
            if formatted != source {
                eprintln!("stdin would be reformatted");
            }
            return Ok(formatted == source);
        }
        io::stdout()
            .write_all(formatted.as_bytes())
            .map_err(|error| format!("stdout: {error}"))?;
        return Ok(true);
    }
    let mut files = Vec::new();
    for path in paths {
        collect(&path, &mut files).map_err(|error| error.to_string())?;
    }
    files.sort();
    files.dedup();
    let mut clean = true;
    for path in &files {
        let source = fs::read_to_string(path).map_err(|error| at(path, error))?;
        let language = if path.extension().is_some_and(|ext| ext == "json") {
            Language::Json
        } else {
            Language::Zql
        };
        let formatted = language
            .format(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if formatted == source {
            continue;
        }
        clean = false;
        if check {
            println!("{} would be reformatted", path.display());
        } else {
            fs::write(path, formatted).map_err(|error| at(path, error))?;
            println!("formatted {}", path.display());
        }
    }
    eprintln!("checked {} ZQL/JSON file(s)", files.len());
    Ok(!check || clean)
}

fn at(path: &Path, error: io::Error) -> String {
    format!("{}: {error}", path.display())
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let context = |error: io::Error| io::Error::new(error.kind(), at(path, error));
    let metadata = fs::symlink_metadata(path).map_err(context)?;
    // Never follow a directory link into another checkout (or a cycle).
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other(format!(
            "refusing to rewrite symbolic link: {}",
            path.display()
        )));
    }
    if metadata.is_file() {
        out.push(path.to_owned());
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(context)? {
        let entry = entry.map_err(context)?;
        let kind = entry.file_type().map_err(context)?;
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
        } else if entry
            .path()
            .extension()
            .is_some_and(|ext| ext == "zql" || ext == "json")
        {
            out.push(entry.path());
        }
    }
    Ok(())
}
