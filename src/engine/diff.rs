//! Reconciliation and Diff Engine: detects creations, renames, moves, and deletions.

use crate::buffer::{BufferLine, EntryId, ItemMeta};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// A single proposed filesystem change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsAction {
    CreateFile { path: PathBuf },
    CreateDir { path: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf, is_dir: bool },
}

impl FsAction {
    pub fn description(&self) -> String {
        match self {
            FsAction::CreateFile { path } => {
                format!("+ CREATE FILE  {}", path.display())
            }
            FsAction::CreateDir { path } => {
                format!("+ CREATE DIR   {}/", path.display())
            }
            FsAction::Rename { from, to } => {
                let from_name = from.file_name().unwrap_or_default().to_string_lossy();
                let to_name = to.file_name().unwrap_or_default().to_string_lossy();
                format!("~ RENAME       {} -> {}", from_name, to_name)
            }
            FsAction::Move { from, to } => {
                format!("→ MOVE         {} -> {}", from.display(), to.display())
            }
            FsAction::Delete { path, is_dir } => {
                let suffix = if *is_dir { "/" } else { "" };
                format!("- DELETE       {}{}", path.display(), suffix)
            }
        }
    }
}

/// A collection of ordered filesystem actions.
#[derive(Debug, Clone, Default)]
pub struct FsPlan {
    pub actions: Vec<FsAction>,
}

impl FsPlan {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }
}

/// Compare the baseline snapshot against current buffer lines and generate a safe, ordered `FsPlan`.
pub fn compute_diff(
    current_dir: &Path,
    baseline: &BTreeMap<EntryId, (String, ItemMeta)>,
    lines: &[BufferLine],
) -> Result<FsPlan> {
    let mut actions = Vec::new();

    // 1. Detect Deletions: any EntryId in baseline missing from live lines
    let active_ids: HashSet<EntryId> = lines.iter().filter_map(|l| l.entry_id).collect();

    for (id, (name, meta)) in baseline {
        if !active_ids.contains(id) {
            let path = current_dir.join(name);
            actions.push(FsAction::Delete {
                path,
                is_dir: meta.file_type == crate::buffer::FileType::Directory,
            });
        }
    }

    // 2. Detect Creates, Renames, and Moves from active lines
    let mut renames = Vec::new();
    let mut moves = Vec::new();
    let mut creates = Vec::new();

    for line in lines {
        let text = line.clean_name();
        if text.is_empty() {
            continue; // Ignore blank lines
        }

        match line.entry_id {
            None => {
                // New line added by user -> Create
                let target_path = current_dir.join(text);
                if line.is_dir() {
                    creates.push(FsAction::CreateDir { path: target_path });
                } else {
                    creates.push(FsAction::CreateFile { path: target_path });
                }
            }
            Some(id) => {
                if let Some((original_name, _)) = baseline.get(&id) {
                    if text != original_name {
                        let from = current_dir.join(original_name);
                        let to = current_dir.join(text);

                        // If text contains a directory separator, it's a move across directories
                        if text.contains('/') {
                            moves.push(FsAction::Move { from, to });
                        } else {
                            renames.push(FsAction::Rename { from, to });
                        }
                    }
                }
            }
        }
    }

    // 3. Collision Checks: Verify no target conflicts with existing files not being vacated
    let vacated_paths: HashSet<PathBuf> = actions
        .iter()
        .filter_map(|a| match a {
            FsAction::Delete { path, .. } => Some(path.clone()),
            _ => None,
        })
        .chain(renames.iter().map(|a| match a {
            FsAction::Rename { from, .. } => from.clone(),
            _ => unreachable!(),
        }))
        .chain(moves.iter().map(|a| match a {
            FsAction::Move { from, .. } => from.clone(),
            _ => unreachable!(),
        }))
        .collect();

    for action in creates.iter().chain(renames.iter()).chain(moves.iter()) {
        let target = match action {
            FsAction::CreateFile { path } | FsAction::CreateDir { path } => path,
            FsAction::Rename { to, .. } | FsAction::Move { to, .. } => to,
            _ => continue,
        };

        if target.exists() && !vacated_paths.contains(target) {
            bail!(
                "Collision detected: Destination already exists: {:?}",
                target
            );
        }
    }

    // 4. Cycle Detection in Renames (e.g. A -> B and B -> A)
    let ordered_renames = resolve_rename_cycles(renames)?;

    // 5. Build final ordered action list:
    // Order: Dirs to create -> Staged/ordered renames -> Moves -> Creates -> Deletions
    let (create_dirs, create_files): (Vec<_>, Vec<_>) = creates
        .into_iter()
        .partition(|a| matches!(a, FsAction::CreateDir { .. }));

    actions.extend(create_dirs);
    actions.extend(ordered_renames);
    actions.extend(moves);
    actions.extend(create_files);

    Ok(FsPlan { actions })
}

/// Break rename cycles (such as A -> B and B -> A) using intermediate temporary filenames.
fn resolve_rename_cycles(renames: Vec<FsAction>) -> Result<Vec<FsAction>> {
    let mut result = Vec::new();
    let mut rename_map: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();

    for r in &renames {
        if let FsAction::Rename { from, to } = r {
            rename_map.insert(from.clone(), to.clone());
        }
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut staged_swaps: Vec<FsAction> = Vec::new();

    for (from, to) in &rename_map {
        if visited.contains(from) {
            continue;
        }

        // Trace chain
        let mut chain = Vec::new();
        let mut curr = from.clone();

        while let Some(next) = rename_map.get(&curr) {
            chain.push(curr.clone());
            visited.insert(curr.clone());
            if next == from {
                // Cycle detected! E.g. A -> B -> A
                let temp_name = format!("{}.__euler_swap_tmp__", from.display());
                let temp_path = PathBuf::from(temp_name);

                staged_swaps.push(FsAction::Rename {
                    from: from.clone(),
                    to: temp_path.clone(),
                });

                // Intermediate renames
                for pair in chain.windows(2).rev() {
                    staged_swaps.push(FsAction::Rename {
                        from: pair[0].clone(),
                        to: pair[1].clone(),
                    });
                }

                // Final swap step
                staged_swaps.push(FsAction::Rename {
                    from: temp_path,
                    to: to.clone(),
                });
                break;
            }
            if visited.contains(next) {
                break;
            }
            curr = next.clone();
        }
    }

    if !staged_swaps.is_empty() {
        result.extend(staged_swaps);
    } else {
        result.extend(renames);
    }

    Ok(result)
}
