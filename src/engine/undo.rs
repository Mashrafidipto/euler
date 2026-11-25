//! Filesystem Undo/Redo Engine.

use anyhow::Result;
use std::fs;
use std::path::PathBuf;

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
                    let _ = fs::rename(to, from);
                }
            }
            UndoAction::Trashed { original_path, trash_path } => {
                if trash_path.exists() {
                    if let Some(parent) = original_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::rename(trash_path, original_path);
                }
            }
        }
        Ok(())
    }
}

/// Stack maintaining filesystem transaction history.
#[derive(Debug, Default)]
pub struct UndoStack {
    pub history: Vec<Vec<UndoAction>>,
    pub redo_history: Vec<Vec<UndoAction>>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successfully executed batch of actions.
    pub fn push(&mut self, batch: Vec<UndoAction>) {
        if !batch.is_empty() {
            self.history.push(batch);
            self.redo_history.clear();
        }
    }

    /// Revert the last applied batch.
    pub fn undo(&mut self) -> Result<bool> {
        if let Some(batch) = self.history.pop() {
            for action in batch.iter().rev() {
                action.revert()?;
            }
            self.redo_history.push(batch);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Re-apply the last undone batch.
    pub fn redo(&mut self) -> Result<bool> {
        if let Some(batch) = self.redo_history.pop() {
            for action in &batch {
                // To redo:
                match action {
                    UndoAction::CreatedFile(path) => {
                        let _ = fs::File::create(path);
                    }
                    UndoAction::CreatedDir(path) => {
                        let _ = fs::create_dir_all(path);
                    }
                    UndoAction::Renamed { from, to } => {
                        let _ = fs::rename(from, to);
                    }
                    UndoAction::Trashed { original_path, trash_path } => {
                        let _ = fs::rename(original_path, trash_path);
                    }
                }
            }
            self.history.push(batch);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
