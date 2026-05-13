use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;

pub fn read_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_string())
}

pub fn read_parse<T: FromStr, P: AsRef<Path>>(path: P) -> Option<T> {
    read_string(path).ok().and_then(|s| s.parse().ok())
}

pub fn read_bytes<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>> {
    fs::read(path)
}

pub fn dir_entries<P: AsRef<Path>>(path: P) -> Vec<std::path::PathBuf> {
    fs::read_dir(path)
        .map(|it| it.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}
