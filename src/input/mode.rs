//! Application operating modes.

use std::path::PathBuf;

/// The current interactive mode of Euler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Normal Mode: Vim motions, line ops, directory navigation.
    Normal,

    /// Insert Mode: direct in-buffer text editing of filenames.
    Insert,

    /// Visual Line Mode: multi-line selection for bulk delete/yank.
    VisualLine { start_row: usize },

    /// Command Mode: bottom command bar for `:w`, `:q`, etc.
    Command { input: String },

    /// Search Mode: bottom search prompt for `/pattern`.
    Search { input: String },

    /// Diff Confirmation Modal: review pending changes before writing to disk.
    ConfirmModal { scroll: usize },

    /// Help Modal: quick reference cheat sheet.
    HelpModal { scroll: usize },

    /// Dirty Navigation Guard: prompted when trying to navigate away with unsaved edits.
    DirtyNavPrompt { target: PathBuf, is_parent: bool },
}

impl Mode {
    /// Display name of the active mode for the status bar.
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::VisualLine { .. } => "VISUAL LINE",
            Mode::Command { .. } => "COMMAND",
            Mode::Search { .. } => "SEARCH",
            Mode::ConfirmModal { .. } => "CONFIRM",
            Mode::HelpModal { .. } => "HELP",
            Mode::DirtyNavPrompt { .. } => "UNSAVED EDITS",
        }
    }
}
