//! Derive a SQL-friendly table name from a file path.
//!
//! Open files register in the shared session under a human-readable name so the
//! SQL Workspace can tell them apart. We sanitize the file stem into a valid
//! identifier; when that yields nothing usable (e.g. a name that's all
//! punctuation), we fall back to a deterministic three-word handle
//! (what3words-style) seeded by hashing the path, so reopening the same file
//! always yields the same name.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Derive a unique, valid table name for `path`, avoiding any name in `existing`.
pub fn derive_table_name(path: &Path, existing: &HashSet<String>) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let base = sanitize(stem).unwrap_or_else(|| three_words(path));
    make_unique(base, existing)
}

/// Lowercase, map non-`[a-z0-9_]` to `_`, collapse repeats, trim `_`, and ensure
/// it doesn't start with a digit. Returns `None` if nothing usable remains.
fn sanitize(stem: &str) -> Option<String> {
    let mut out = String::with_capacity(stem.len());
    let mut last_underscore = false;
    for ch in stem.chars() {
        let c = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };
        if c == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        out.push(c);
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        return None;
    }
    let mut name = trimmed.to_string();
    // SQL identifiers can't start with a digit.
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name.insert_str(0, "t_");
    }
    Some(name)
}

/// Append `_2`, `_3`, … until the name is free.
fn make_unique(base: String, existing: &HashSet<String>) -> String {
    if !existing.contains(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}_{n}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// A deterministic `adjective_adjective_noun` handle derived from the path hash.
fn three_words(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let h = hasher.finish();
    let a = ADJECTIVES[(h % ADJECTIVES.len() as u64) as usize];
    let b = ADJECTIVES[((h / 97) % ADJECTIVES.len() as u64) as usize];
    let c = NOUNS[((h / 9173) % NOUNS.len() as u64) as usize];
    format!("{a}_{b}_{c}")
}

const ADJECTIVES: &[&str] = &[
    "amber", "brave", "calm", "clever", "crimson", "eager", "fuzzy", "gentle", "golden", "happy",
    "jolly", "keen", "lively", "mellow", "noble", "olive", "proud", "quiet", "rapid", "scarlet",
    "swift", "teal", "vivid", "witty", "zesty",
];

const NOUNS: &[&str] = &[
    "otter", "falcon", "maple", "comet", "harbor", "lantern", "meadow", "nimbus", "orchid",
    "pebble", "quartz", "river", "summit", "thicket", "willow",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn name(path: &str, existing: &[&str]) -> String {
        let set: HashSet<String> = existing.iter().map(|s| s.to_string()).collect();
        derive_table_name(Path::new(path), &set)
    }

    #[test]
    fn sanitizes_common_names() {
        assert_eq!(name("/data/trades.parquet", &[]), "trades");
        assert_eq!(name("/data/My Orders.parquet", &[]), "my_orders");
        assert_eq!(
            name("/data/us-futures.2024.parquet", &[]),
            "us_futures_2024"
        );
    }

    #[test]
    fn digit_leading_names_get_prefixed() {
        assert_eq!(name("/data/2024.parquet", &[]), "t_2024");
    }

    #[test]
    fn collisions_get_numeric_suffixes() {
        assert_eq!(name("/a/trades.parquet", &["trades"]), "trades_2");
        assert_eq!(
            name("/a/trades.parquet", &["trades", "trades_2"]),
            "trades_3"
        );
    }

    #[test]
    fn unsanitizable_names_fall_back_to_words() {
        // All-punctuation stem → deterministic three-word handle.
        let n = name("/data/!!!.parquet", &[]);
        assert_eq!(n.split('_').count(), 3, "expected three words, got {n}");
        // Deterministic for the same path.
        assert_eq!(n, name("/data/!!!.parquet", &[]));
        // Result is a valid identifier (no leading digit, ascii word chars).
        assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    }
}
