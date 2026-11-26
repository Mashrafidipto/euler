//! The DirectoryBuffer: in-memory editable representation of a directory.

use super::line::{BufferLine, EntryId, FileType, ItemMeta};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// In-memory representation of a directory as an editable text buffer.
#[derive(Debug, Clone)]
pub struct DirectoryBuffer {
    /// The directory path currently being viewed.
    pub current_dir: PathBuf,

    /// Snapshot of filesystem entries when the directory was loaded.
    /// Key: EntryId -> Value: (Original Filename, ItemMeta)
    pub baseline: BTreeMap<EntryId, (String, ItemMeta)>,

    /// Active editable lines in the buffer.
    pub lines: Vec<BufferLine>,

    /// Cursor position in the buffer (0-indexed).
    pub cursor_row: usize,
    pub cursor_col: usize,

    /// Vertical scroll offset for terminal viewport rendering.
    pub scroll_offset: usize,

    /// Counter used to issue unique EntryIds.
    pub next_id: usize,
}

impl DirectoryBuffer {
    /// Load directory contents from disk into a fresh DirectoryBuffer.
    pub fn load(path: &Path, show_hidden: bool, natural_sort: bool) -> Result<Self> {
        let abs_path = path
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize path: {:?}", path))?;

        let mut lines = Vec::new();
        let mut baseline = BTreeMap::new();
        let mut next_id = 1;

        let mut entries: Vec<_> = fs::read_dir(&abs_path)
            .with_context(|| format!("Failed to read directory: {:?}", abs_path))?
            .flatten()
            .collect();

        // Sort entries
        if natural_sort {
            entries.sort_by(|a, b| {
                let name_a = a.file_name().to_string_lossy().to_string();
                let name_b = b.file_name().to_string_lossy().to_string();
                natord::compare(&name_a, &name_b)
            });
        } else {
            entries.sort_by_key(|e| e.file_name());
        }

        for entry in entries {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Filter hidden files if not enabled
            if !show_hidden && file_name.starts_with('.') {
                continue;
            }

            let path = entry.path();
            let symlink_metadata = fs::symlink_metadata(&path).ok();
            let metadata = fs::metadata(&path).ok();

            let file_type = if let Some(sym) = &symlink_metadata {
                if sym.file_type().is_symlink() {
                    FileType::Symlink
                } else if sym.file_type().is_dir() {
                    FileType::Directory
                } else {
                    FileType::File
                }
            } else if let Some(meta) = &metadata {
                if meta.is_dir() {
                    FileType::Directory
                } else {
                    FileType::File
                }
            } else {
                FileType::File
            };

            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = metadata.and_then(|m| m.modified().ok());
            let symlink_target = if file_type == FileType::Symlink {
                fs::read_link(&path).ok()
            } else {
                None
            };

            #[cfg(unix)]
            let permissions = {
                use std::os::unix::fs::PermissionsExt;
                symlink_metadata
                    .map(|m| m.permissions().mode())
                    .unwrap_or(0o644)
            };
            #[cfg(not(unix))]
            let permissions = 0o644;

            let meta = ItemMeta {
                file_type,
                size,
                permissions,
                modified,
                symlink_target,
            };

            let entry_id = EntryId(next_id);
            next_id += 1;

            baseline.insert(entry_id, (file_name.clone(), meta.clone()));
            lines.push(BufferLine::from_disk(entry_id, file_name, path, meta));
        }

        Ok(Self {
            current_dir: abs_path,
            baseline,
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            next_id,
        })
    }

    /// Reload the current directory from disk, discarding uncommitted buffer edits.
    pub fn reload(&mut self, show_hidden: bool, natural_sort: bool) -> Result<()> {
        let fresh = Self::load(&self.current_dir, show_hidden, natural_sort)?;
        self.baseline = fresh.baseline;
        self.lines = fresh.lines;
        self.next_id = fresh.next_id;
        self.clamp_cursor_normal();
        Ok(())
    }

    /// Check if the buffer has any uncommitted changes compared to the baseline.
    pub fn is_dirty(&self) -> bool {
        // If line count differs, it's dirty
        if self.lines.len() != self.baseline.len() {
            return true;
        }

        // Check if any line was added, moved from elsewhere, or modified
        for line in &self.lines {
            if let Some(ref src) = line.source_path {
                let expected = self.current_dir.join(line.clean_name());
                if src != &expected {
                    return true; // Moved from a different path
                }
            }

            match line.entry_id {
                None => return true, // Brand new line or pasted without local entry_id
                Some(id) => {
                    if let Some((original_name, _)) = self.baseline.get(&id) {
                        if &line.text != original_name {
                            return true; // Renamed or moved
                        }
                    } else {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Clamp cursor to valid row and column boundaries.
    pub fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.cursor_row = 0;
            self.cursor_col = 0;
            return;
        }

        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len() - 1;
        }

        let line_len = self.lines[self.cursor_row].text.chars().count();
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    /// Adjust horizontal cursor for Normal mode (where cursor can't sit past last char unless empty).
    pub fn clamp_cursor_normal(&mut self) {
        if self.lines.is_empty() {
            self.cursor_row = 0;
            self.cursor_col = 0;
            return;
        }

        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len() - 1;
        }

        let line_len = self.lines[self.cursor_row].text.chars().count();
        if line_len == 0 {
            self.cursor_col = 0;
        } else if self.cursor_col >= line_len {
            self.cursor_col = line_len - 1;
        }
    }

    /// Get reference to current line.
    pub fn current_line(&self) -> Option<&BufferLine> {
        self.lines.get(self.cursor_row)
    }

    /// Get mutable reference to current line.
    pub fn current_line_mut(&mut self) -> Option<&mut BufferLine> {
        self.lines.get_mut(self.cursor_row)
    }

    // --- In-Buffer Editing Methods ---

    /// Insert a character at the cursor position in the current line.
    pub fn insert_char(&mut self, c: char) {
        if self.lines.is_empty() {
            self.lines.push(BufferLine::new_empty());
            self.cursor_row = 0;
            self.cursor_col = 0;
        }
        self.clamp_cursor();

        let col = self.cursor_col;
        let line = &mut self.lines[self.cursor_row];
        let mut chars: Vec<char> = line.text.chars().collect();
        let idx = col.min(chars.len());
        chars.insert(idx, c);
        line.text = chars.into_iter().collect();
        self.cursor_col = idx + 1;
    }

    /// Delete character before cursor (Backspace).
    pub fn delete_char_before(&mut self) {
        if self.cursor_col == 0 || self.lines.is_empty() || self.cursor_row >= self.lines.len() {
            return;
        }

        let line = &mut self.lines[self.cursor_row];
        let mut chars: Vec<char> = line.text.chars().collect();
        let col = self.cursor_col.min(chars.len());
        if col > 0 {
            chars.remove(col - 1);
            line.text = chars.into_iter().collect();
            self.cursor_col = col - 1;
        }
    }

    /// Delete character under cursor (Delete key or 'x' in Normal mode).
    pub fn delete_char_under(&mut self) {
        if self.lines.is_empty() || self.cursor_row >= self.lines.len() {
            return;
        }

        let col = self.cursor_col;
        let line = &mut self.lines[self.cursor_row];
        let mut chars: Vec<char> = line.text.chars().collect();
        if col < chars.len() {
            chars.remove(col);
            line.text = chars.into_iter().collect();
        }
    }

    /// Delete previous word in current line (Ctrl-w in Insert mode).
    pub fn delete_word_before(&mut self) {
        if self.cursor_col == 0 || self.lines.is_empty() || self.cursor_row >= self.lines.len() {
            return;
        }

        let line = &mut self.lines[self.cursor_row];
        let chars: Vec<char> = line.text.chars().collect();
        let col = self.cursor_col.min(chars.len());

        let mut idx = col;
        // Skip trailing spaces
        while idx > 0 && chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        // Delete back to start of word
        while idx > 0 && !chars[idx - 1].is_whitespace() {
            idx -= 1;
        }

        let mut new_chars = chars[..idx].to_vec();
        new_chars.extend_from_slice(&chars[col..]);
        line.text = new_chars.into_iter().collect();
        self.cursor_col = idx;
    }

    /// Clear from line start to cursor (Ctrl-u in Insert mode).
    pub fn clear_to_start(&mut self) {
        if self.cursor_col == 0 || self.lines.is_empty() || self.cursor_row >= self.lines.len() {
            return;
        }

        let line = &mut self.lines[self.cursor_row];
        let chars: Vec<char> = line.text.chars().collect();
        let col = self.cursor_col.min(chars.len());
        line.text = chars[col..].iter().collect();
        self.cursor_col = 0;
    }

    /// Insert empty line below current line (Vim 'o' or Enter in insert mode).
    pub fn insert_line_below(&mut self) {
        let insert_idx = if self.lines.is_empty() {
            0
        } else {
            (self.cursor_row + 1).min(self.lines.len())
        };

        self.lines.insert(insert_idx, BufferLine::new_empty());
        self.cursor_row = insert_idx;
        self.cursor_col = 0;
    }

    /// Insert empty line above current line (Vim 'O').
    pub fn insert_line_above(&mut self) {
        let insert_idx = self.cursor_row.min(self.lines.len());
        self.lines.insert(insert_idx, BufferLine::new_empty());
        self.cursor_row = insert_idx;
        self.cursor_col = 0;
    }

    /// Delete current line (Vim 'dd'). Returns the deleted line.
    pub fn delete_line(&mut self) -> Option<BufferLine> {
        if self.lines.is_empty() || self.cursor_row >= self.lines.len() {
            return None;
        }

        let removed = self.lines.remove(self.cursor_row);
        if self.cursor_row >= self.lines.len() && !self.lines.is_empty() {
            self.cursor_row = self.lines.len() - 1;
        }
        self.clamp_cursor_normal();
        Some(removed)
    }

    /// Clear text from cursor to end of line (Vim 'D').
    pub fn delete_to_end_of_line(&mut self) {
        if self.lines.is_empty() || self.cursor_row >= self.lines.len() {
            return;
        }

        let col = self.cursor_col;
        let line = &mut self.lines[self.cursor_row];
        let chars: Vec<char> = line.text.chars().collect();
        if col < chars.len() {
            line.text = chars[..col].iter().collect();
        }
        self.clamp_cursor_normal();
    }
}
