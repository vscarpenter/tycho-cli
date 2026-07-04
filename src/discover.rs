//! Discovery of transcript files under one or more search roots.
//!
//! Layouts vary across Claude Code versions (see `docs/SCHEMA.md`), so
//! discovery is a recursive walk for `*.jsonl` files at any depth — never a
//! fixed nesting pattern. Filtering out non-transcript JSONL (e.g. workflow
//! `journal.jsonl`) happens later, by record type, not by path.

use std::path::{Path, PathBuf};

/// A transcript file found under a search root, labeled with the encoded
/// project directory it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TranscriptFile {
    /// Encoded project directory name, e.g. `-Users-vinny-Projects-gsd`.
    /// Files sitting directly in a root get the placeholder `(root)`.
    pub project: String,
    /// Path to the `.jsonl` file.
    pub path: PathBuf,
}

/// Compute the default search roots from the home directory and the
/// `CLAUDE_CONFIG_DIR` environment value (passed in, not read here, so the
/// function stays pure and testable).
///
/// When `claude_config_dir` is set it is a comma-separated list of config
/// roots that *replaces* `~/.claude`; each entry contributes
/// `<entry>/projects`. The macOS Xcode bonus location is always appended —
/// it is independent of the config dir and simply absent on other machines.
pub fn default_roots(home: &Path, claude_config_dir: Option<&str>) -> Vec<PathBuf> {
    let mut roots = match claude_config_dir {
        Some(dirs) => dirs
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| Path::new(entry).join("projects"))
            .collect(),
        None => vec![home.join(".claude/projects")],
    };
    roots.push(home.join("Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"));
    roots
}

/// Recursively find every `*.jsonl` file under the given roots.
///
/// Roots that do not exist are skipped silently (the Xcode location is
/// usually absent). Results are sorted by project, then path, so output is
/// deterministic regardless of filesystem iteration order.
pub fn discover(roots: &[PathBuf]) -> Vec<TranscriptFile> {
    let mut files: Vec<TranscriptFile> = roots
        .iter()
        .flat_map(|root| {
            walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
                .map(|entry| TranscriptFile {
                    project: project_name(root, entry.path()),
                    path: entry.into_path(),
                })
        })
        .collect();
    files.sort();
    files
}

/// The first directory component below the root is the encoded project name.
fn project_name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .ok()
        .and_then(|rel| rel.parent()?.components().next())
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "(root)".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build the three transcript layouts observed in real data, plus
    /// decoy files that must not (memory notes) or must (journal.jsonl)
    /// be discovered.
    fn fixture_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let proj = root.join("-Users-v-Projects-gsd");
        fs::create_dir_all(proj.join("sess-1/subagents/workflows/wf_1")).unwrap();
        fs::create_dir_all(proj.join("memory")).unwrap();
        fs::write(proj.join("sess-1.jsonl"), "{}\n").unwrap();
        fs::write(proj.join("sess-1/subagents/agent-a1.jsonl"), "{}\n").unwrap();
        fs::write(
            proj.join("sess-1/subagents/workflows/wf_1/agent-a2.jsonl"),
            "{}\n",
        )
        .unwrap();
        fs::write(
            proj.join("sess-1/subagents/workflows/wf_1/journal.jsonl"),
            "{}\n",
        )
        .unwrap();
        fs::write(proj.join("memory/MEMORY.md"), "not a transcript").unwrap();
        dir
    }

    #[test]
    fn finds_jsonl_at_every_observed_nesting_depth() {
        let dir = fixture_tree();
        let found = discover(&[dir.path().to_path_buf()]);
        let names: Vec<_> = found
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"sess-1.jsonl".to_string()));
        assert!(names.contains(&"agent-a1.jsonl".to_string()));
        assert!(names.contains(&"agent-a2.jsonl".to_string()));
    }

    #[test]
    fn includes_journal_jsonl_and_excludes_non_jsonl() {
        let dir = fixture_tree();
        let found = discover(&[dir.path().to_path_buf()]);
        let names: Vec<_> = found
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // journal.jsonl is filtered later by record type, not by path.
        assert!(names.contains(&"journal.jsonl".to_string()));
        assert!(!names.iter().any(|n| n.ends_with(".md")));
    }

    #[test]
    fn labels_every_file_with_the_project_directory_name() {
        let dir = fixture_tree();
        let found = discover(&[dir.path().to_path_buf()]);
        assert!(!found.is_empty());
        assert!(found.iter().all(|f| f.project == "-Users-v-Projects-gsd"));
    }

    #[test]
    fn nonexistent_root_is_skipped_silently() {
        let found = discover(&[PathBuf::from("/definitely/not/a/real/root")]);
        assert!(found.is_empty());
    }

    #[test]
    fn merges_multiple_roots_in_deterministic_order() {
        let a = fixture_tree();
        let b = tempfile::tempdir().unwrap();
        let proj_b = b.path().join("-Users-v-other");
        fs::create_dir_all(&proj_b).unwrap();
        fs::write(proj_b.join("sess-9.jsonl"), "{}\n").unwrap();

        let forward = discover(&[a.path().to_path_buf(), b.path().to_path_buf()]);
        let reverse = discover(&[b.path().to_path_buf(), a.path().to_path_buf()]);
        assert_eq!(forward.len(), 5);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn default_roots_without_config_dir_uses_home_claude_and_xcode() {
        let roots = default_roots(Path::new("/Users/v"), None);
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/Users/v/.claude/projects"),
                PathBuf::from(
                    "/Users/v/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"
                ),
            ]
        );
    }

    #[test]
    fn default_roots_with_config_dir_replaces_home_claude() {
        let roots = default_roots(Path::new("/Users/v"), Some("/cfg/a, /cfg/b,"));
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/cfg/a/projects"),
                PathBuf::from("/cfg/b/projects"),
                PathBuf::from(
                    "/Users/v/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"
                ),
            ]
        );
    }
}
