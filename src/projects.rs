use std::fs;
use std::path::{Path, PathBuf};

/// Discover recent projects from Claude Code's history.
/// Returns decoded paths sorted by most recently modified.
pub fn discover_projects() -> Vec<String> {
    let Some(home) = std::env::var("HOME").ok() else {
        return Vec::new();
    };
    let projects_dir = PathBuf::from(&home).join(".claude/projects");
    if !projects_dir.is_dir() {
        return Vec::new();
    }

    let mut entries: Vec<(String, std::time::SystemTime)> = Vec::new();

    if let Ok(dir) = fs::read_dir(&projects_dir) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('-') {
                continue;
            }
            if let Some(path) = decode_project_path(&name) {
                if Path::new(&path).is_dir() {
                    let modified = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    entries.push((path, modified));
                }
            }
        }
    }

    // Sort by most recently modified first.
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.into_iter().map(|(path, _)| path).collect()
}

/// Decode a Claude Code project directory name back to a filesystem path.
///
/// Claude encodes `/Users/nkyos/my-project` as `-Users-nkyos-my-project`.
/// We greedily resolve each `-` as `/` or literal `-` by checking filesystem existence.
fn decode_project_path(encoded: &str) -> Option<String> {
    let stripped = encoded.strip_prefix('-')?;
    let parts: Vec<&str> = stripped.split('-').collect();
    if parts.is_empty() {
        return None;
    }

    let mut path = format!("/{}", parts[0]);

    for &part in &parts[1..] {
        let try_slash = format!("{path}/{part}");
        let try_dash = format!("{path}-{part}");

        if Path::new(&try_slash).exists() {
            path = try_slash;
        } else if Path::new(&try_dash).exists() {
            path = try_dash;
        } else {
            // Optimistically assume `/`
            path = try_slash;
        }
    }

    Some(path)
}

/// Directories to skip during recursive search (large or generated).
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "__pycache__",
    "vendor",
    "venv",
    "Pods",
    "DerivedData",
    "Library",
];

/// Recursively search for directories whose name matches the filter.
/// Returns relative paths from `dir`, sorted alphabetically.
pub fn find_dirs_recursive(dir: &str, filter: &str, max_depth: usize, max_results: usize) -> Vec<String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Vec::new();
    }
    let lower_filter = filter.to_lowercase();
    let mut results = Vec::new();
    find_dirs_inner(root, "", &lower_filter, max_depth, max_results, &mut results);
    results
}

fn find_dirs_inner(
    base: &Path,
    relative: &str,
    filter: &str,
    depth: usize,
    max_results: usize,
    results: &mut Vec<String>,
) {
    if depth == 0 || results.len() >= max_results {
        return;
    }

    let Ok(entries) = fs::read_dir(base) else {
        return;
    };

    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        dirs.push((name, entry.path()));
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, full_path) in dirs {
        if results.len() >= max_results {
            return;
        }

        let rel_path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };

        // Match against directory name only (not full path) for cleaner results.
        if name.to_lowercase().contains(filter) {
            results.push(rel_path.clone());
        }

        find_dirs_inner(&full_path, &rel_path, filter, depth - 1, max_results, results);
    }
}

/// List directories in the given path for the directory browser.
pub fn list_dirs(dir: &str) -> Vec<String> {
    let path = Path::new(dir);
    if !path.is_dir() {
        return Vec::new();
    }

    let mut dirs: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().into_owned();
                // Skip hidden directories.
                if !name.starts_with('.') {
                    dirs.push(name);
                }
            }
        }
    }
    dirs.sort();
    dirs
}
