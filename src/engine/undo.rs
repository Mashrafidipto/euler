//! Filesystem Undo/Redo Engine.

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Move a file or directory across paths, falling back to copy+remove on EXDEV (errno 18).
fn move_path_safe(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Err(e) = fs::rename(from, to) {
        if e.raw_os_error() == Some(18) {
            if from.is_dir() {
                copy_dir_recursive(from, to)?;
                fs::remove_dir_all(from)?;
            } else {
                fs::copy(from, to)?;
                fs::remove_file(from)?;
            }
        } else {
            return Err(e.into());
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
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

/// A reversible action recorded during plan execution.
#[derive(Debug, Clone)]
pub enum UndoAction {
    CreatedFile(PathBuf),
    CreatedDir(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Trashed { original_path: PathBuf, trash_path: PathBuf },
}

impl UndoAction {
    /// Reverse this action.
    pub fn revert(&self) -> Result<()> {
        match self {
            UndoAction::CreatedFile(path) => {
                if path.exists() {
                    let _ = fs::remove_file(path);
                }
            }
            UndoAction::CreatedDir(path) => {
                if path.exists() {
                    let _ = fs::remove_dir_all(path);
                }
            }
            UndoAction::Renamed { from, to } => {
                if to.exists() {
                    let _ = move_path_safe(to, from);
                }
            }
            UndoAction::Trashed { original_path, trash_path } => {
                if trash_path.exists() {
                    let _ = move_path_safe(trash_path, original_path);
                }
            }
        }
        Ok(())
    }

    /// Re-apply a previously reverted action (used for global filesystem redo).
    pub fn reapply(&self) -> Result<()> {
        match self {
            UndoAction::CreatedFile(path) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                if !path.exists() {
                    fs::File::create(path)?;
                }
            }
            UndoAction::CreatedDir(path) => {
                fs::create_dir_all(path)?;
            }
            UndoAction::Renamed { from, to } => {
                if from.exists() {
                    move_path_safe(from, to)?;
                }
            }
            UndoAction::Trashed { original_path, trash_path } => {
                if original_path.exists() {
                    move_path_safe(original_path, trash_path)?;
                }
            }
        }
        Ok(())
    }
}
