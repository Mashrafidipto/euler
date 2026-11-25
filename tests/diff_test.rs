//! Unit tests for Euler diff and reconciliation engine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Mock structures to test diff algorithm independently
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct EntryId(usize);

#[derive(Debug, Clone)]
struct MockLine {
    entry_id: Option<EntryId>,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MockAction {
    CreateFile(PathBuf),
    CreateDir(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

fn compute_mock_diff(
    current_dir: &Path,
    baseline: &BTreeMap<EntryId, String>,
    lines: &[MockLine],
) -> Vec<MockAction> {
    let mut actions = Vec::new();
    let active_ids: std::collections::HashSet<EntryId> = lines.iter().filter_map(|l| l.entry_id).collect();

    // 1. Deletions
    for (id, name) in baseline {
        if !active_ids.contains(id) {
            actions.push(MockAction::Delete(current_dir.join(name)));
        }
    }

    // 2. Creates and Renames
    for line in lines {
        let text = line.text.trim_end_matches('/');
        if text.is_empty() {
            continue;
        }

        match line.entry_id {
            None => {
                if line.text.ends_with('/') {
                    actions.push(MockAction::CreateDir(current_dir.join(text)));
                } else {
                    actions.push(MockAction::CreateFile(current_dir.join(text)));
                }
            }
            Some(id) => {
                if let Some(orig) = baseline.get(&id) {
                    if text != orig {
                        actions.push(MockAction::Rename {
                            from: current_dir.join(orig),
                            to: current_dir.join(text),
                        });
                    }
                }
            }
        }
    }

    actions
}

#[test]
fn test_detects_creation() {
    let dir = PathBuf::from("/test");
    let baseline = BTreeMap::new();
    let lines = vec![
        MockLine { entry_id: None, text: "new_file.txt".to_string() },
        MockLine { entry_id: None, text: "new_dir/".to_string() },
    ];

    let actions = compute_mock_diff(&dir, &baseline, &lines);
    assert_eq!(actions.len(), 2);
    assert!(actions.contains(&MockAction::CreateFile(dir.join("new_file.txt"))));
    assert!(actions.contains(&MockAction::CreateDir(dir.join("new_dir"))));
}

#[test]
fn test_detects_deletion() {
    let dir = PathBuf::from("/test");
    let mut baseline = BTreeMap::new();
    baseline.insert(EntryId(1), "keep.txt".to_string());
    baseline.insert(EntryId(2), "remove.txt".to_string());

    let lines = vec![
        MockLine { entry_id: Some(EntryId(1)), text: "keep.txt".to_string() },
    ];

    let actions = compute_mock_diff(&dir, &baseline, &lines);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0], MockAction::Delete(dir.join("remove.txt")));
}

#[test]
fn test_detects_rename() {
    let dir = PathBuf::from("/test");
    let mut baseline = BTreeMap::new();
    baseline.insert(EntryId(1), "old_name.rs".to_string());

    let lines = vec![
        MockLine { entry_id: Some(EntryId(1)), text: "new_name.rs".to_string() },
    ];

    let actions = compute_mock_diff(&dir, &baseline, &lines);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0],
        MockAction::Rename {
            from: dir.join("old_name.rs"),
            to: dir.join("new_name.rs"),
        }
    );
}
