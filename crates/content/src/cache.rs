//! `.ron` loading with a modification-time validated bincode cache.

use crate::registry::{ContentRegistry, HasId};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, std::io::Error),
    Ron(PathBuf, String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            LoadError::Ron(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for LoadError {}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Cache file path for a source file: `{cache_dir}/{source}__{stem}.bin`.
pub fn cache_path(cache_dir: &Path, source: &str, ron_path: &Path) -> PathBuf {
    let stem = ron_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    cache_dir.join(format!("{}__{}.bin", sanitize(source), sanitize(&stem)))
}

/// Cache is valid when the `.bin` exists and is not older than the source file.
pub fn is_cache_valid(ron_path: &Path, bin_path: &Path) -> bool {
    let (Ok(src), Ok(bin)) = (
        std::fs::metadata(ron_path).and_then(|m| m.modified()),
        std::fs::metadata(bin_path).and_then(|m| m.modified()),
    ) else {
        return false;
    };
    bin >= src
}

/// Load one `.ron` file, using the cache when valid and refreshing it otherwise.
pub fn load_file<T: Serialize + DeserializeOwned>(
    ron_path: &Path,
    bin_path: &Path,
) -> Result<T, LoadError> {
    if is_cache_valid(ron_path, bin_path) {
        if let Ok(bytes) = std::fs::read(bin_path) {
            if let Ok(v) = bincode::deserialize::<T>(&bytes) {
                return Ok(v);
            }
        }
    }
    let text = std::fs::read_to_string(ron_path).map_err(|e| LoadError::Io(ron_path.into(), e))?;
    let value: T =
        ron::from_str(&text).map_err(|e| LoadError::Ron(ron_path.into(), e.to_string()))?;
    // Caching is best-effort; failure to write must not fail loading.
    if let Ok(bytes) = bincode::serialize(&value) {
        if let Some(parent) = bin_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(bin_path, bytes);
    }
    Ok(value)
}

/// Load every `.ron` in `dir` (sorted by name) into `reg`; a missing dir is skipped.
pub fn load_dir_into<T: HasId + Serialize + DeserializeOwned>(
    reg: &mut ContentRegistry<T>,
    dir: &Path,
    source: &str,
    cache_dir: &Path,
) -> Result<(), LoadError> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ron"))
        .collect();
    files.sort();
    for f in files {
        let v: T = load_file(&f, &cache_path(cache_dir, source, &f))?;
        reg.insert(v.id().to_string(), v);
    }
    Ok(())
}
