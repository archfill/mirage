// Ignore pattern support (.mirageignore).
//
// Reads gitignore-style patterns from a file and provides
// path matching to exclude entries from sync.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Compiled ignore rules loaded from a .mirageignore file.
pub struct IgnoreRules {
    globset: GlobSet,
}

impl IgnoreRules {
    /// Load ignore patterns from the given file path.
    ///
    /// Returns an empty rule set if the file does not exist.
    /// Each non-empty, non-comment line is treated as a glob pattern.
    pub fn load(path: &Path) -> Self {
        let mut builder = GlobSetBuilder::new();

        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                // Support both "*.tmp" (basename match) and "path/to/*.tmp" patterns
                match Glob::new(trimmed) {
                    Ok(glob) => {
                        builder.add(glob);
                    }
                    Err(e) => {
                        tracing::warn!(pattern = trimmed, error = %e, "invalid ignore pattern");
                    }
                }
                // Also add a variant prefixed with **/ for basename-only patterns
                if !trimmed.contains('/')
                    && let Ok(glob) = Glob::new(&format!("**/{trimmed}"))
                {
                    builder.add(glob);
                }
            }
        }

        let globset = builder.build().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to build ignore globset");
            GlobSet::empty()
        });

        Self { globset }
    }

    /// Check if a path should be ignored.
    pub fn is_ignored(&self, path: &str) -> bool {
        self.globset.is_match(path)
    }

    /// Returns true if no patterns are loaded.
    pub fn is_empty(&self) -> bool {
        self.globset.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_ignores_nothing() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "").unwrap();
        let rules = IgnoreRules::load(tmp.path());
        assert!(!rules.is_ignored("anything.txt"));
        assert!(rules.is_empty());
    }

    #[test]
    fn missing_file_ignores_nothing() {
        let rules = IgnoreRules::load(Path::new("/nonexistent/.mirageignore"));
        assert!(!rules.is_ignored("anything"));
        assert!(rules.is_empty());
    }

    #[test]
    fn glob_pattern_matches() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "*.tmp\n*.log\n").unwrap();
        let rules = IgnoreRules::load(tmp.path());
        assert!(rules.is_ignored("test.tmp"));
        assert!(rules.is_ignored("subdir/test.tmp"));
        assert!(rules.is_ignored("deep/nested/file.log"));
        assert!(!rules.is_ignored("test.txt"));
    }

    #[test]
    fn comments_and_blank_lines_skipped() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "# comment\n\n*.bak\n  # another comment\n").unwrap();
        let rules = IgnoreRules::load(tmp.path());
        assert!(rules.is_ignored("file.bak"));
        assert!(!rules.is_ignored("file.txt"));
    }

    #[test]
    fn directory_pattern() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), ".git\nnode_modules\n").unwrap();
        let rules = IgnoreRules::load(tmp.path());
        assert!(rules.is_ignored(".git"));
        assert!(rules.is_ignored("subdir/.git"));
        assert!(rules.is_ignored("node_modules"));
    }
}
