//! Discovery of transcript files under one or more search roots.
//!
//! Layouts vary across Claude Code versions (see `docs/SCHEMA.md`), so
//! discovery is a recursive walk for `*.jsonl` files at any depth — never a
//! fixed nesting pattern. Filtering out non-transcript JSONL (e.g. workflow
//! `journal.jsonl`) happens later, by record type, not by path.

use std::path::{Path, PathBuf};

/// Which product wrote the files under a search root. Assigned per root at
/// discovery time; `External` (from `--dir`) is the only provider whose
/// files are format-sniffed line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provider {
    Claude,
    Codex,
    External,
}

/// A search root plus the provider that owns its layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
    pub path: PathBuf,
    pub provider: Provider,
}

/// A transcript file found under a search root, labeled with the encoded
/// project directory it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TranscriptFile {
    /// Encoded project directory name, e.g. `-Users-vinny-Projects-gsd`.
    /// Files sitting directly in a root get the placeholder `(root)`.
    pub project: String,
    /// Path to the `.jsonl` file.
    pub path: PathBuf,
    /// The provider that owns this file's search root.
    pub provider: Provider,
}

/// Compute the default search roots from the home directory plus the Claude
/// and Codex config environment values (passed in, not read here, so the
/// function stays pure and testable).
///
/// When `claude_config_dir` is set it is a comma-separated list of config
/// roots that *replaces* `~/.claude`; each entry contributes
/// `<entry>/projects`. The macOS Xcode bonus location is always appended —
/// it is independent of the config dir and simply absent on other machines.
///
/// Codex contributes two roots, both under `<CODEX_HOME>` when set and
/// `~/.codex` otherwise: `sessions` for live rollouts and
/// `archived_sessions`, where Codex moves a rollout when its thread is
/// archived. The archived file keeps the same name and record shape, so
/// omitting that root would silently drop the spend it recorded.
pub fn default_roots(
    home: &Path,
    claude_config_dir: Option<&str>,
    codex_home: Option<&str>,
) -> Vec<SearchRoot> {
    let claude_paths: Vec<PathBuf> = match claude_config_dir {
        Some(dirs) => dirs
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| Path::new(entry).join("projects"))
            .collect(),
        // Joined per component so Windows renders native separators: a
        // single join(".claude/projects") resolves fine but prints as
        // `C:\Users\x\.claude/projects`, which reads like a bug in `doctor`.
        None => vec![home.join(".claude").join("projects")],
    };
    let mut roots: Vec<SearchRoot> = claude_paths
        .into_iter()
        .map(|path| SearchRoot {
            path,
            provider: Provider::Claude,
        })
        .collect();
    roots.push(SearchRoot {
        path: [
            "Library",
            "Developer",
            "Xcode",
            "CodingAssistant",
            "ClaudeAgentConfig",
            "projects",
        ]
        .iter()
        .fold(home.to_path_buf(), |path, segment| path.join(segment)),
        provider: Provider::Claude,
    });
    let codex_base = codex_home
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    for dir in ["sessions", "archived_sessions"] {
        roots.push(SearchRoot {
            path: codex_base.join(dir),
            provider: Provider::Codex,
        });
    }
    roots
}

/// Windows profile directories that never belong to a real user, so a
/// `.claude` under them would not be a person's transcript store.
const RESERVED_WINDOWS_PROFILES: [&str; 4] = ["Public", "Default", "Default User", "All Users"];

/// Whether this process is running inside WSL, judged from
/// `/proc/sys/kernel/osrelease` and `WSL_DISTRO_NAME`. Pure so the decision
/// is testable without a WSL machine.
pub fn is_wsl(osrelease: Option<&str>, wsl_distro_name: Option<&str>) -> bool {
    if wsl_distro_name.is_some_and(|name| !name.trim().is_empty()) {
        return true;
    }
    osrelease.is_some_and(|release| {
        let release = release.to_ascii_lowercase();
        release.contains("microsoft") || release.contains("wsl")
    })
}

/// Claude Code project roots belonging to *Windows* user profiles, as seen
/// from inside WSL, that are not already being scanned.
///
/// Claude Code resolves its config directory from the running environment's
/// home — `CLAUDE_CONFIG_DIR`, else `homedir()/.claude`, with no
/// platform-specific branch — so running `claude` on Windows and again inside
/// WSL produces two independent transcript stores. A Linux-native tycho only
/// sees the WSL one. The Windows one is reachable through the
/// `/mnt/<drive>/Users` interop mount, but it is not a default root and its
/// spend would otherwise be invisible with no indication anything was missed.
///
/// `mnt` is a parameter rather than a hardcoded `/mnt` so the walk is
/// testable on any platform.
pub fn windows_claude_roots(mnt: &Path, scanned: &[SearchRoot]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(drives) = std::fs::read_dir(mnt) else {
        return found;
    };
    for drive in drives.filter_map(Result::ok) {
        let Ok(profiles) = std::fs::read_dir(drive.path().join("Users")) else {
            continue;
        };
        for profile in profiles.filter_map(Result::ok) {
            let name = profile.file_name();
            let name = name.to_string_lossy();
            if RESERVED_WINDOWS_PROFILES
                .iter()
                .any(|reserved| reserved.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            let candidate = profile.path().join(".claude").join("projects");
            if candidate.is_dir() && !scanned.iter().any(|root| root.path == candidate) {
                found.push(candidate);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Recursively find every `*.jsonl` file under the given roots.
///
/// Roots that do not exist are skipped silently (the Xcode location is
/// usually absent). Results are sorted by project, then path, so output is
/// deterministic regardless of filesystem iteration order.
pub fn discover(roots: &[SearchRoot]) -> Vec<TranscriptFile> {
    let mut files: Vec<TranscriptFile> = roots
        .iter()
        .flat_map(|root| {
            walkdir::WalkDir::new(&root.path)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
                .map(|entry| TranscriptFile {
                    project: project_name(&root.path, root.provider, entry.path()),
                    path: entry.into_path(),
                    provider: root.provider,
                })
        })
        .collect();
    files.sort();
    files
}

/// The first directory component below the root is the encoded project
/// name for Claude-layout roots. Codex roots are date-sharded
/// (`sessions/<yyyy>/<mm>/<dd>/`), so their files get the `(codex)`
/// placeholder; the real project comes from record metadata during the scan.
fn project_name(root: &Path, provider: Provider, file: &Path) -> String {
    if provider == Provider::Codex {
        return "(codex)".to_owned();
    }
    file.strip_prefix(root)
        .ok()
        .and_then(|rel| rel.parent()?.components().next())
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "(root)".to_owned())
}

/// Replicate Claude Code's project-directory encoding ('/' and '.' become
/// '-'). Lossy by design; applied to Codex cwd values so one repo shows as
/// one project row regardless of provider.
pub fn encode_project(path: &str) -> String {
    path.replace(['/', '.'], "-")
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
        let found = discover(&[SearchRoot {
            path: dir.path().to_path_buf(),
            provider: Provider::Claude,
        }]);
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
        let found = discover(&[SearchRoot {
            path: dir.path().to_path_buf(),
            provider: Provider::Claude,
        }]);
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
        let found = discover(&[SearchRoot {
            path: dir.path().to_path_buf(),
            provider: Provider::Claude,
        }]);
        assert!(!found.is_empty());
        assert!(found.iter().all(|f| f.project == "-Users-v-Projects-gsd"));
    }

    #[test]
    fn nonexistent_root_is_skipped_silently() {
        let found = discover(&[SearchRoot {
            path: PathBuf::from("/definitely/not/a/real/root"),
            provider: Provider::Claude,
        }]);
        assert!(found.is_empty());
    }

    #[test]
    fn merges_multiple_roots_in_deterministic_order() {
        let a = fixture_tree();
        let b = tempfile::tempdir().unwrap();
        let proj_b = b.path().join("-Users-v-other");
        fs::create_dir_all(&proj_b).unwrap();
        fs::write(proj_b.join("sess-9.jsonl"), "{}\n").unwrap();

        let root_a = SearchRoot {
            path: a.path().to_path_buf(),
            provider: Provider::Claude,
        };
        let root_b = SearchRoot {
            path: b.path().to_path_buf(),
            provider: Provider::Claude,
        };
        let forward = discover(&[root_a.clone(), root_b.clone()]);
        let reverse = discover(&[root_b, root_a]);
        assert_eq!(forward.len(), 5);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn default_roots_tag_providers() {
        let roots = default_roots(Path::new("/Users/v"), None, None);
        assert_eq!(
            roots,
            vec![
                SearchRoot {
                    path: PathBuf::from("/Users/v/.claude/projects"),
                    provider: Provider::Claude
                },
                SearchRoot {
                    path: PathBuf::from(
                        "/Users/v/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"
                    ),
                    provider: Provider::Claude
                },
                SearchRoot {
                    path: PathBuf::from("/Users/v/.codex/sessions"),
                    provider: Provider::Codex
                },
                SearchRoot {
                    path: PathBuf::from("/Users/v/.codex/archived_sessions"),
                    provider: Provider::Codex
                },
            ]
        );
    }

    /// On Unix a single `join("a/b")` and `join("a").join("b")` compare
    /// equal, so the equality tests above cannot catch a regression here —
    /// only the component structure can. A path built with an embedded `/`
    /// still *resolves* on Windows but prints with mixed separators.
    #[test]
    fn default_roots_are_built_one_component_at_a_time() {
        let roots = default_roots(Path::new("/Users/v"), None, None);
        // Only Normal components: the root/prefix component is legitimately
        // the separator itself ("/" on Unix, "C:\" on Windows).
        for root in &roots {
            for component in root.path.components() {
                let std::path::Component::Normal(text) = component else {
                    continue;
                };
                let text = text.to_string_lossy();
                assert!(
                    !text.contains('/') && !text.contains('\\'),
                    "component {text:?} in {:?} embeds a separator",
                    root.path
                );
            }
        }
        let tail: Vec<_> = roots[0]
            .path
            .components()
            .rev()
            .take(2)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(tail, vec!["projects".to_owned(), ".claude".to_owned()]);
    }

    #[test]
    fn default_roots_with_config_dir_replaces_home_claude() {
        let roots = default_roots(
            Path::new("/Users/v"),
            Some("/cfg/a, /cfg/b,"),
            Some("/codex"),
        );
        assert_eq!(
            roots,
            vec![
                SearchRoot {
                    path: PathBuf::from("/cfg/a/projects"),
                    provider: Provider::Claude
                },
                SearchRoot {
                    path: PathBuf::from("/cfg/b/projects"),
                    provider: Provider::Claude
                },
                SearchRoot {
                    path: PathBuf::from(
                        "/Users/v/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"
                    ),
                    provider: Provider::Claude
                },
                SearchRoot {
                    path: PathBuf::from("/codex/sessions"),
                    provider: Provider::Codex
                },
                SearchRoot {
                    path: PathBuf::from("/codex/archived_sessions"),
                    provider: Provider::Codex
                },
            ]
        );
    }

    #[test]
    fn empty_codex_home_falls_back_to_home_codex() {
        // A set-but-empty CODEX_HOME must not yield relative "sessions" or
        // "archived_sessions" roots.
        for value in ["", "   "] {
            let roots = default_roots(Path::new("/Users/v"), None, Some(value));
            for expected in [
                "/Users/v/.codex/sessions",
                "/Users/v/.codex/archived_sessions",
            ] {
                assert!(roots.iter().any(|r| r.path == Path::new(expected)));
            }
        }
    }

    /// Codex moves completed rollouts out of `sessions/` when a thread is
    /// archived; the file keeps its name and shape, so missing this root
    /// silently drops that spend from every report.
    #[test]
    fn archived_codex_rollouts_are_discovered_like_live_ones() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join("rollout-2026-05-07T13-19-01-019e03aa-1ec7.jsonl"),
            "{}\n",
        )
        .unwrap();
        let found = discover(&[SearchRoot {
            path: dir.path().to_path_buf(),
            provider: Provider::Codex,
        }]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project, "(codex)");
        assert_eq!(found[0].provider, Provider::Codex);
    }

    #[test]
    fn wsl_is_detected_from_osrelease_or_distro_name() {
        assert!(is_wsl(Some("5.15.153.1-microsoft-standard-WSL2"), None));
        assert!(is_wsl(Some("6.6.0-MICROSOFT-standard"), None)); // case-insensitive
        assert!(is_wsl(None, Some("Ubuntu-24.04")));
        assert!(!is_wsl(Some("6.8.0-45-generic"), None)); // ordinary Linux
        assert!(!is_wsl(None, None));
        assert!(!is_wsl(None, Some("   "))); // set-but-empty is not WSL
    }

    /// Build a fake `/mnt` interop tree: one real profile with transcripts,
    /// one reserved profile that must be ignored, and one profile with no
    /// `.claude` at all.
    fn wsl_mnt_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for profile in ["vinny", "Public"] {
            fs::create_dir_all(
                dir.path()
                    .join("c/Users")
                    .join(profile)
                    .join(".claude/projects"),
            )
            .unwrap();
        }
        fs::create_dir_all(dir.path().join("c/Users/no-claude")).unwrap();
        dir
    }

    #[test]
    fn windows_profiles_visible_from_wsl_are_reported() {
        let mnt = wsl_mnt_fixture();
        let found = windows_claude_roots(mnt.path(), &[]);
        assert_eq!(
            found,
            vec![mnt.path().join("c/Users/vinny/.claude/projects")]
        );
    }

    #[test]
    fn windows_roots_already_scanned_are_not_reported_again() {
        let mnt = wsl_mnt_fixture();
        let scanned = [SearchRoot {
            path: mnt.path().join("c/Users/vinny/.claude/projects"),
            provider: Provider::Claude,
        }];
        assert!(windows_claude_roots(mnt.path(), &scanned).is_empty());
    }

    #[test]
    fn missing_mnt_is_not_an_error() {
        assert!(windows_claude_roots(Path::new("/definitely/not/a/mount"), &[]).is_empty());
    }

    #[test]
    fn codex_root_files_get_placeholder_project() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/07/08");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-x.jsonl"), "{}\n").unwrap();
        let found = discover(&[SearchRoot {
            path: dir.path().to_path_buf(),
            provider: Provider::Codex,
        }]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project, "(codex)");
        assert_eq!(found[0].provider, Provider::Codex);
    }
}
