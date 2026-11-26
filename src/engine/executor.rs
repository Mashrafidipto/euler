//! Filesystem Action Executor: applies FsPlan changes atomically to disk.

use super::diff::{FsAction, FsPlan};
use super::undo::UndoAction;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Execute all actions in the given plan.
/// Returns a list of UndoActions for rolling back changes.
pub fn execute_plan(plan: &FsPlan, trash_deletes: bool) -> Result<Vec<UndoAction>> {
    let mut undo_log = Vec::new();

    for action in &plan.actions {
        match action {
            FsAction::CreateDir { path } => {
                if !path.exists() {
                    fs::create_dir_all(path)
                        .with_context(|| format!("Failed to create directory: {:?}", path))?;
                    undo_log.push(UndoAction::CreatedDir(path.clone()));
                }
            }

            FsAction::CreateFile { path } => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("Failed to create parent dirs for: {:?}", path))?;
                }
                fs::File::create(path)
                    .with_context(|| format!("Failed to create file: {:?}", path))?;
                undo_log.push(UndoAction::CreatedFile(path.clone()));
            }

            FsAction::Rename { from, to } | FsAction::Move { from, to } => {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("Failed to create parent dirs for: {:?}", to))?;
                }
                if let Err(e) = fs::rename(from, to) {
                    // Fallback for cross-device moves (EXDEV = 18 on Unix)
                    if e.raw_os_error() == Some(18) {
                        if from.is_dir() {
                            copy_dir_recursive(from, to)?;
                            fs::remove_dir_all(from)
                                .with_context(|| format!("Failed to remove source dir after cross-device move: {:?}", from))?;
                        } else {
                            fs::copy(from, to)
                                .with_context(|| format!("Failed to copy file across devices: {:?} to {:?}", from, to))?;
                            fs::remove_file(from)
                                .with_context(|| format!("Failed to remove source file after cross-device move: {:?}", from))?;
                        }
                    } else {
                        return Err(e).with_context(|| format!("Failed to rename {:?} to {:?}", from, to));
                    }
                }
                undo_log.push(UndoAction::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                });
            }

            FsAction::Delete { path, is_dir } => {
                if !path.exists() {
                    continue;
                }

                if trash_deletes {
                    let trash_dest = move_to_trash(path)?;
                    undo_log.push(UndoAction::Trashed {
                        original_path: path.clone(),
                        trash_path: trash_dest,
                    });
                } else if *is_dir {
                    fs::remove_dir_all(path)
                        .with_context(|| format!("Failed to remove directory: {:?}", path))?;
                } else {
                    fs::remove_file(path)
                        .with_context(|| format!("Failed to remove file: {:?}", path))?;
                }
            }
        }
    }

    Ok(undo_log)
}

/// Move a file or directory to the user's system trash following the XDG trash spec.
fn move_to_trash(target: &Path) -> Result<PathBuf> {
    let trash_dir = get_trash_dir()?;
    let files_dir = trash_dir.join("files");
    let info_dir = trash_dir.join("info");

    fs::create_dir_all(&files_dir)?;
    fs::create_dir_all(&info_dir)?;

    let file_name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    // Avoid trash collisions with unique timestamp suffix if needed
    let mut dest_file = files_dir.join(&file_name);
    let mut base_name = file_name.clone();
    let mut counter = 1;
    while dest_file.exists() {
        base_name = format!("{}.{}", file_name, counter);
        dest_file = files_dir.join(&base_name);
        counter += 1;
    }

    // Move to trash/files (with cross-device EXDEV fallback)
    if let Err(e) = fs::rename(target, &dest_file) {
        if e.raw_os_error() == Some(18) {
            if target.is_dir() {
                copy_dir_recursive(target, &dest_file)?;
                fs::remove_dir_all(target)
                    .with_context(|| format!("Failed to remove source dir after trash move: {:?}", target))?;
            } else {
                fs::copy(target, &dest_file)
                    .with_context(|| format!("Failed to copy file to trash: {:?}", target))?;
                fs::remove_file(target)
                    .with_context(|| format!("Failed to remove source file after trash move: {:?}", target))?;
            }
        } else {
            return Err(e).with_context(|| format!("Failed to move {:?} to trash", target));
        }
    }

    // Write .trashinfo file
    let info_file = info_dir.join(format!("{}.trashinfo", base_name));
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let info_content = format!(
        "[Trash Info]\nPath={}\nDeletionTimestamp={}\n",
        target.canonicalize().unwrap_or_else(|_| target.to_path_buf()).display(),
        timestamp
    );
    let _ = fs::write(info_file, info_content);

    Ok(dest_file)
}

/// Retrieve the XDG trash directory ($XDG_DATA_HOME/Trash or ~/.local/share/Trash).
fn get_trash_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg).join("Trash"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".local/share/Trash"));
    }
    Ok(PathBuf::from("/tmp/Trash"))
}

/// Recursively copy a directory tree (used as fallback for cross-device moves).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else {
            fs::copy(&entry_path, &dest_path)?;
        }
    }
    Ok(())
}
