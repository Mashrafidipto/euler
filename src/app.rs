//! Application controller: owns multi-column directory state and handles user input.

use crate::buffer::{BufferLine, DirectoryBuffer};
use crate::config::Config;
use crate::engine::{compute_diff, execute_plan, FsAction, FsPlan, UndoStack};
use crate::input::Mode;
use crate::ui::Terminal;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const MAX_COLUMNS: usize = 4;

/// An item stored in the Euler clipboard from a delete (cut) or yank (copy) operation.
#[derive(Debug, Clone)]
pub struct ClipboardItem {
    pub line: BufferLine,
    pub is_cut: bool,
}

pub struct App {
    /// Active directory columns open side-by-side (up to 4 in a row like mini.files).
    pub columns: Vec<DirectoryBuffer>,

    /// Index of the currently focused column (0 <= active_col < columns.len()).
    pub active_col: usize,

    pub config: Config,
    pub mode: Mode,
    pub pending_plan: Option<FsPlan>,
    pub undo_stack: UndoStack,
    pub clipboard: Vec<ClipboardItem>,
    pub search_query: Option<String>,
    pub status_msg: Option<(String, Instant)>,
    pub should_quit: bool,
    pub quit_after_confirm: bool,
}

impl App {
    /// Initialize Euler on a target directory, loading multi-column preview.
    pub fn new(path: &Path, config: Config) -> Result<Self> {
        let root_buffer = DirectoryBuffer::load(path, config.show_hidden, config.natural_sort)?;
        let mut app = Self {
            columns: vec![root_buffer],
            active_col: 0,
            config,
            mode: Mode::Normal,
            pending_plan: None,
            undo_stack: UndoStack::new(),
            clipboard: Vec::new(),
            search_query: None,
            status_msg: None,
            should_quit: false,
            quit_after_confirm: false,
        };

        // Open child directory in the next column if cursor is on a directory
        app.ensure_child_preview();

        Ok(app)
    }

    /// Reference to the currently active directory buffer.
    pub fn active_buffer(&self) -> &DirectoryBuffer {
        &self.columns[self.active_col]
    }

    /// Mutable reference to the currently active directory buffer.
    pub fn active_buffer_mut(&mut self) -> &mut DirectoryBuffer {
        &mut self.columns[self.active_col]
    }

    /// Set a temporary status message to be displayed in the bottom bar.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some((msg.into(), Instant::now()));
    }

    /// Check if the status message has expired (after 4 seconds).
    fn current_status(&mut self) -> Option<String> {
        if let Some((_, time)) = &self.status_msg {
            if time.elapsed() >= Duration::from_secs(4) {
                self.status_msg = None;
            }
        }
        self.status_msg.as_ref().map(|(msg, _)| msg.clone())
    }

    /// Ensure the column to the right shows the directory under the active cursor.
    fn ensure_child_preview(&mut self) {
        if self.active_col >= self.columns.len() {
            return;
        }

        let child_path_opt = {
            let active = &self.columns[self.active_col];
            if let Some(line) = active.current_line() {
                if line.is_dir() && !line.clean_name().is_empty() {
                    Some(active.current_dir.join(line.clean_name()))
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(child_path) = child_path_opt {
            if child_path.is_dir() {
                let next_col = self.active_col + 1;
                if next_col < self.columns.len() {
                    // Update next column if its path changed and it's not modified
                    if self.columns[next_col].current_dir != child_path
                        && !self.columns[next_col].is_dirty()
                    {
                        if let Ok(buf) = DirectoryBuffer::load(
                            &child_path,
                            self.config.show_hidden,
                            self.config.natural_sort,
                        ) {
                            self.columns[next_col] = buf;
                            // Clean up stale grandchild columns if not modified
                            if next_col + 1 < self.columns.len()
                                && !self.columns[next_col + 1].is_dirty()
                            {
                                self.columns.truncate(next_col + 1);
                            }
                        }
                    }
                } else if self.columns.len() < MAX_COLUMNS {
                    // Open as new adjacent column
                    if let Ok(buf) = DirectoryBuffer::load(
                        &child_path,
                        self.config.show_hidden,
                        self.config.natural_sort,
                    ) {
                        self.columns.push(buf);
                    }
                }
            }
        }
        // Note: When cursor is on a file (child_path_opt is None), do NOT close
        // or truncate adjacent columns. Keep open folders viewable side-by-side.
        // Adjacent columns are only updated when the cursor moves to a new folder.
    }

    /// Main application loop with event-driven rendering.
    pub fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        let mut needs_render = true;

        while !self.should_quit {
            let status_msg = self.current_status();

            if needs_render {
                terminal.cursorline = self.config.cursorline;
                terminal.render(
                    &mut self.columns,
                    self.active_col,
                    &self.mode,
                    self.pending_plan.as_ref(),
                    status_msg.as_deref(),
                )?;
                needs_render = false;
            }

            let poll_duration = if self.status_msg.is_some() {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(60)
            };

            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            if let Err(e) = self.handle_key(key, terminal) {
                                self.set_status(format!("Error: {}", e));
                                self.mode = Mode::Normal;
                            }
                            needs_render = true;
                        }
                    }
                    Event::Resize(_, _) => {
                        needs_render = true;
                    }
                    _ => {}
                }
            } else if self.status_msg.is_some() {
                needs_render = true;
            }
        }

        Ok(())
    }

    /// Dispatch keyboard events based on the active mode.
    fn handle_key(&mut self, key: KeyEvent, terminal: &mut Terminal) -> Result<()> {
        // Enforce rule: Explicitly disable and ignore ALL arrow keys
        if self.config.disable_arrow_keys {
            match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                    return Ok(());
                }
                _ => {}
            }
        }

        match &self.mode {
            Mode::Normal => self.handle_normal_mode(key, terminal)?,
            Mode::Insert => self.handle_insert_mode(key)?,
            Mode::VisualLine { start_row } => {
                let start = *start_row;
                self.handle_visual_mode(key, start)?;
            }
            Mode::Command { input } => {
                let input_str = input.clone();
                self.handle_command_mode(key, input_str)?;
            }
            Mode::Search { input } => {
                let input_str = input.clone();
                self.handle_search_mode(key, input_str)?;
            }
            Mode::ConfirmModal { scroll } => {
                let s = *scroll;
                self.handle_confirm_modal(key, s)?;
            }
            Mode::HelpModal { scroll } => {
                let s = *scroll;
                self.handle_help_modal(key, s)?;
            }
        }

        Ok(())
    }

    // --- Mode Handlers ---

    /// Normal Mode event dispatching.
    fn handle_normal_mode(&mut self, key: KeyEvent, terminal: &mut Terminal) -> Result<()> {
        match key.code {
            // Move to previous / parent column (left) without requiring save!
            KeyCode::Char('h') => {
                if self.active_col > 0 {
                    // Move focus to left column; edits in current column are preserved in memory!
                    self.active_col -= 1;
                } else {
                    // Leftmost column: load parent directory as new leftmost column
                    let parent_opt = self.columns[0]
                        .current_dir
                        .parent()
                        .map(|p| p.to_path_buf());

                    if let Some(parent_path) = parent_opt {
                        let current_folder_name = self.columns[0]
                            .current_dir
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();

                        if let Ok(mut parent_buf) = DirectoryBuffer::load(
                            &parent_path,
                            self.config.show_hidden,
                            self.config.natural_sort,
                        ) {
                            // Position cursor in parent onto the folder we just came from
                            if let Some(pos) = parent_buf
                                .lines
                                .iter()
                                .position(|l| l.clean_name() == current_folder_name)
                            {
                                parent_buf.cursor_row = pos;
                                parent_buf.clamp_cursor_normal();
                            }

                            self.columns.insert(0, parent_buf);
                            // Keep max columns within bounds if rightmost column is not modified
                            if self.columns.len() > MAX_COLUMNS
                                && self.columns.last().map(|c| !c.is_dirty()).unwrap_or(false)
                            {
                                self.columns.pop();
                            }
                            self.active_col = 0;
                        }
                    } else {
                        self.set_status("Already at root directory.");
                    }
                }
            }

            // Move to child column (right) or open file with xdg-open
            KeyCode::Char('l') => {
                let active = &self.columns[self.active_col];
                if let Some(line) = active.current_line() {
                    let target_path = active.current_dir.join(line.clean_name());
                    if line.is_dir() {
                        let next_col = self.active_col + 1;
                        if next_col < self.columns.len() {
                            // Focus existing child column
                            self.active_col += 1;
                            self.ensure_child_preview();
                        } else if let Ok(buf) = DirectoryBuffer::load(
                            &target_path,
                            self.config.show_hidden,
                            self.config.natural_sort,
                        ) {
                            self.columns.push(buf);
                            self.active_col += 1;
                            self.ensure_child_preview();
                        }
                    } else {
                        // Open file in system opener (xdg-open)
                        self.open_with_system_opener()?;
                    }
                }
            }

            // Move to child column (right) or open file in $EDITOR
            KeyCode::Enter => {
                let active = &self.columns[self.active_col];
                if let Some(line) = active.current_line() {
                    let target_path = active.current_dir.join(line.clean_name());
                    if line.is_dir() {
                        let next_col = self.active_col + 1;
                        if next_col < self.columns.len() {
                            // Focus existing child column
                            self.active_col += 1;
                            self.ensure_child_preview();
                        } else if let Ok(buf) = DirectoryBuffer::load(
                            &target_path,
                            self.config.show_hidden,
                            self.config.natural_sort,
                        ) {
                            self.columns.push(buf);
                            self.active_col += 1;
                            self.ensure_child_preview();
                        }
                    } else {
                        // Open file in user's $EDITOR
                        self.open_in_editor(&target_path, terminal)?;
                    }
                }
            }

            // Hyphen is disabled by specification
            KeyCode::Char('-') => {}

            // Delete character under cursor: 'x'
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::NONE) => {
                self.active_buffer_mut().push_undo_snapshot();
                self.active_buffer_mut().delete_char_under();
            }

            // 'g' prefix chords
            KeyCode::Char('g') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press {
                        match next.code {
                            KeyCode::Char('g') => {
                                self.active_buffer_mut().cursor_row = 0;
                                self.active_buffer_mut().clamp_cursor_normal();
                                self.ensure_child_preview();
                            }
                            KeyCode::Char('.') => {
                                self.config.show_hidden = !self.config.show_hidden;
                                // Reload clean columns with new hidden setting
                                for col in &mut self.columns {
                                    if !col.is_dirty() {
                                        let _ = col.reload(
                                            self.config.show_hidden,
                                            self.config.natural_sort,
                                        );
                                    }
                                }
                                let state = if self.config.show_hidden {
                                    "visible"
                                } else {
                                    "hidden"
                                };
                                self.set_status(format!("Dotfiles are now {}", state));
                                self.ensure_child_preview();
                            }
                            KeyCode::Char('x') => {
                                self.open_with_system_opener()?;
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Vertical Motions: 'j' and 'k'
            KeyCode::Char('j') => {
                let total = self.active_buffer().lines.len();
                if total > 0 {
                    self.active_buffer_mut().cursor_row =
                        (self.active_buffer().cursor_row + 1).min(total - 1);
                    self.active_buffer_mut().clamp_cursor_normal();
                    self.ensure_child_preview();
                }
            }
            KeyCode::Char('k') => {
                self.active_buffer_mut().cursor_row =
                    self.active_buffer().cursor_row.saturating_sub(1);
                self.active_buffer_mut().clamp_cursor_normal();
                self.ensure_child_preview();
            }

            // Horizontal Motions: 'w', 'b', '0', '$'
            KeyCode::Char('w') => {
                let col = self.active_buffer().cursor_col;
                if let Some(line) = self.active_buffer().current_line() {
                    let chars: Vec<char> = line.text.chars().collect();
                    let mut new_col = col;
                    while new_col < chars.len() && !chars[new_col].is_whitespace() {
                        new_col += 1;
                    }
                    while new_col < chars.len() && chars[new_col].is_whitespace() {
                        new_col += 1;
                    }
                    self.active_buffer_mut().cursor_col = new_col;
                    self.active_buffer_mut().clamp_cursor_normal();
                }
            }
            KeyCode::Char('b') => {
                let col = self.active_buffer().cursor_col;
                if let Some(line) = self.active_buffer().current_line() {
                    let chars: Vec<char> = line.text.chars().collect();
                    if !chars.is_empty() {
                        let mut new_col = col.min(chars.len());
                        if new_col > 0 {
                            new_col -= 1;
                            while new_col > 0 && chars[new_col].is_whitespace() {
                                new_col -= 1;
                            }
                            while new_col > 0 && !chars[new_col - 1].is_whitespace() {
                                new_col -= 1;
                            }
                        }
                        self.active_buffer_mut().cursor_col = new_col;
                    }
                }
            }
            KeyCode::Char('0') => {
                self.active_buffer_mut().cursor_col = 0;
            }
            KeyCode::Char('$') => {
                if let Some(line) = self.active_buffer().current_line() {
                    let len = line.text.chars().count();
                    self.active_buffer_mut().cursor_col = len.saturating_sub(1);
                }
            }
            KeyCode::Char('G') => {
                let total = self.active_buffer().lines.len();
                if total > 0 {
                    self.active_buffer_mut().cursor_row = total - 1;
                    self.active_buffer_mut().clamp_cursor_normal();
                    self.ensure_child_preview();
                }
            }

            // Scrolling: Ctrl-d / Ctrl-u
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let total = self.active_buffer().lines.len();
                if total > 0 {
                    self.active_buffer_mut().cursor_row =
                        (self.active_buffer().cursor_row + 10).min(total - 1);
                    self.active_buffer_mut().clamp_cursor_normal();
                    self.ensure_child_preview();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.active_buffer_mut().cursor_row =
                    self.active_buffer().cursor_row.saturating_sub(10);
                self.active_buffer_mut().clamp_cursor_normal();
                self.ensure_child_preview();
            }

            // Entering Insert Mode
            KeyCode::Char('i') => {
                self.active_buffer_mut().push_undo_snapshot();
                self.mode = Mode::Insert;
            }
            KeyCode::Char('a') => {
                self.active_buffer_mut().push_undo_snapshot();
                if let Some(line) = self.active_buffer().current_line() {
                    let len = line.text.chars().count();
                    self.active_buffer_mut().cursor_col =
                        (self.active_buffer().cursor_col + 1).min(len);
                }
                self.mode = Mode::Insert;
            }
            KeyCode::Char('I') => {
                self.active_buffer_mut().push_undo_snapshot();
                self.active_buffer_mut().cursor_col = 0;
                self.mode = Mode::Insert;
            }
            KeyCode::Char('A') => {
                self.active_buffer_mut().push_undo_snapshot();
                if let Some(line) = self.active_buffer().current_line() {
                    self.active_buffer_mut().cursor_col = line.text.chars().count();
                }
                self.mode = Mode::Insert;
            }
            KeyCode::Char('o') => {
                self.active_buffer_mut().push_undo_snapshot();
                self.active_buffer_mut().insert_line_below();
                self.mode = Mode::Insert;
            }
            KeyCode::Char('O') => {
                self.active_buffer_mut().push_undo_snapshot();
                self.active_buffer_mut().insert_line_above();
                self.mode = Mode::Insert;
            }

            // Change operations: 'c' prefix
            KeyCode::Char('c') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press {
                        match next.code {
                            KeyCode::Char('w') => {
                                self.active_buffer_mut().push_undo_snapshot();
                                let cursor_col = self.active_buffer().cursor_col;
                                if let Some(line) = self.active_buffer_mut().current_line_mut() {
                                    let chars: Vec<char> = line.text.chars().collect();
                                    let start = cursor_col.min(chars.len());
                                    let mut end = start;
                                    while end < chars.len() && !chars[end].is_whitespace() {
                                        end += 1;
                                    }
                                    let mut new_chars = chars[..start].to_vec();
                                    new_chars.extend_from_slice(&chars[end..]);
                                    line.text = new_chars.into_iter().collect();
                                    self.active_buffer_mut().cursor_col = start;
                                }
                                self.mode = Mode::Insert;
                            }
                            KeyCode::Char('c') => {
                                self.active_buffer_mut().push_undo_snapshot();
                                if let Some(line) = self.active_buffer_mut().current_line_mut() {
                                    line.text.clear();
                                }
                                self.active_buffer_mut().cursor_col = 0;
                                self.mode = Mode::Insert;
                            }
                            _ => {}
                        }
                    }
                }
            }
            KeyCode::Char('C') => {
                self.active_buffer_mut().push_undo_snapshot();
                self.active_buffer_mut().delete_to_end_of_line();
                self.mode = Mode::Insert;
            }

            // Deletions: 'dd', 'D'
            KeyCode::Char('d') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press && next.code == KeyCode::Char('d') {
                        self.active_buffer_mut().push_undo_snapshot();
                        if let Some(deleted) = self.active_buffer_mut().delete_line() {
                            self.clipboard = vec![ClipboardItem {
                                line: deleted,
                                is_cut: true,
                            }];
                            self.ensure_child_preview();
                        }
                    }
                }
            }
            KeyCode::Char('D') => {
                self.active_buffer_mut().push_undo_snapshot();
                self.active_buffer_mut().delete_to_end_of_line();
            }

            // Yank & Paste: 'yy', 'p', 'P'
            KeyCode::Char('y') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press && next.code == KeyCode::Char('y') {
                        if let Some(line) = self.active_buffer().current_line() {
                            self.clipboard = vec![ClipboardItem {
                                line: line.clone(),
                                is_cut: false,
                            }];
                            self.set_status("Line yanked");
                        }
                    }
                }
            }
            KeyCode::Char('p') => {
                self.paste_clipboard(true);
            }
            KeyCode::Char('P') => {
                self.paste_clipboard(false);
            }

            // Visual Line Mode: 'V'
            KeyCode::Char('V') => {
                self.mode = Mode::VisualLine {
                    start_row: self.active_buffer().cursor_row,
                };
            }

            // Undo / Redo: 'u', 'Ctrl-r'
            // Prefer undoing uncommitted in-buffer edits first (one edit at a time),
            // then undo committed filesystem transactions.
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::NONE) => {
                if self.active_buffer_mut().undo_edit() {
                    self.ensure_child_preview();
                    self.set_status("Undid buffer edit");
                } else if self.undo_stack.undo()? {
                    self.reload_columns_after_fs_change();
                    self.set_status("Undid last filesystem change");
                } else {
                    self.set_status("Already at oldest change");
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.active_buffer_mut().redo_edit() {
                    self.ensure_child_preview();
                    self.set_status("Redid buffer edit");
                } else if self.undo_stack.redo()? {
                    self.reload_columns_after_fs_change();
                    self.set_status("Redid filesystem change");
                } else {
                    self.set_status("Already at newest change");
                }
            }
            KeyCode::Char('r') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press {
                        if let KeyCode::Char(ch) = next.code {
                            self.active_buffer_mut().push_undo_snapshot();
                            self.active_buffer_mut().delete_char_under();
                            self.active_buffer_mut().insert_char(ch);
                            self.active_buffer_mut().clamp_cursor_normal();
                        }
                    }
                }
            }

            // Command Mode: ':'
            KeyCode::Char(':') => {
                self.mode = Mode::Command {
                    input: String::new(),
                };
            }

            // Search: '/'
            KeyCode::Char('/') => {
                self.mode = Mode::Search {
                    input: String::new(),
                };
            }
            KeyCode::Char('n') => {
                if let Some(query) = self.search_query.clone() {
                    self.jump_to_next_match(&query);
                } else {
                    self.set_status("No previous search query");
                }
            }
            KeyCode::Char('N') => {
                if let Some(query) = self.search_query.clone() {
                    self.jump_to_prev_match(&query);
                } else {
                    self.set_status("No previous search query");
                }
            }

            _ => {}
        }

        Ok(())
    }

    /// Insert Mode event dispatching.
    fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.active_buffer_mut().clamp_cursor_normal();
                self.mode = Mode::Normal;
                self.ensure_child_preview();
            }
            KeyCode::Backspace => {
                self.active_buffer_mut().delete_char_before();
            }
            KeyCode::Delete => {
                self.active_buffer_mut().delete_char_under();
            }
            KeyCode::Enter => {
                self.active_buffer_mut().insert_line_below();
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.active_buffer_mut().delete_word_before();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.active_buffer_mut().clear_to_start();
            }
            KeyCode::Char(c) => {
                self.active_buffer_mut().insert_char(c);
            }
            _ => {}
        }
        Ok(())
    }

    /// Visual Line Mode event dispatching.
    fn handle_visual_mode(&mut self, key: KeyEvent, start_row: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') => {
                let total = self.active_buffer().lines.len();
                if total > 0 {
                    self.active_buffer_mut().cursor_row =
                        (self.active_buffer().cursor_row + 1).min(total - 1);
                }
            }
            KeyCode::Char('k') => {
                self.active_buffer_mut().cursor_row =
                    self.active_buffer().cursor_row.saturating_sub(1);
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                if self.active_buffer().lines.is_empty() {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                self.active_buffer_mut().push_undo_snapshot();
                let max_row = self.active_buffer().lines.len() - 1;
                let cur_row = self.active_buffer().cursor_row.min(max_row);
                let s_row = start_row.min(max_row);
                let (low, high) = if s_row <= cur_row {
                    (s_row, cur_row)
                } else {
                    (cur_row, s_row)
                };

                let mut deleted_items = Vec::new();
                for _ in low..=high {
                    if low < self.active_buffer().lines.len() {
                        let removed = self.active_buffer_mut().lines.remove(low);
                        deleted_items.push(ClipboardItem {
                            line: removed,
                            is_cut: true,
                        });
                    }
                }
                self.clipboard = deleted_items;
                self.active_buffer_mut().cursor_row =
                    low.min(self.active_buffer().lines.len().saturating_sub(1));
                self.active_buffer_mut().clamp_cursor_normal();
                self.mode = Mode::Normal;
                self.ensure_child_preview();
            }
            KeyCode::Char('y') => {
                if !self.active_buffer().lines.is_empty() {
                    let max_row = self.active_buffer().lines.len() - 1;
                    let cur_row = self.active_buffer().cursor_row.min(max_row);
                    let s_row = start_row.min(max_row);
                    let (low, high) = if s_row <= cur_row {
                        (s_row, cur_row)
                    } else {
                        (cur_row, s_row)
                    };

                    self.clipboard = self.active_buffer().lines[low..=high]
                        .iter()
                        .map(|l| ClipboardItem {
                            line: l.clone(),
                            is_cut: false,
                        })
                        .collect();
                    self.set_status(format!("{} lines yanked", self.clipboard.len()));
                }
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    /// Command Mode (:w, :q, :e, etc.).
    fn handle_command_mode(&mut self, key: KeyEvent, mut input: String) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                input.pop();
                if input.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.mode = Mode::Command { input };
                }
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.mode = Mode::Command { input };
            }
            KeyCode::Enter => {
                self.execute_colon_command(&input)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Collect diffs from ALL modified open directories and compute combined plan.
    fn collect_all_diffs(&self) -> Result<FsPlan> {
        let mut combined_actions = Vec::new();

        for col in &self.columns {
            if col.is_dirty() {
                let plan = compute_diff(&col.current_dir, &col.baseline, &col.lines)?;
                combined_actions.extend(plan.actions);
            }
        }

        // Reconcile: If a file or directory is being Moved or Renamed, cancel any Delete
        // action for that same source path so it moves rather than being deleted!
        let moved_from: HashSet<PathBuf> = combined_actions
            .iter()
            .filter_map(|a| match a {
                FsAction::Move { from, .. } | FsAction::Rename { from, .. } => Some(from.clone()),
                _ => None,
            })
            .collect();

        combined_actions.retain(|a| match a {
            FsAction::Delete { path, .. } => !moved_from.contains(path),
            _ => true,
        });

        Ok(FsPlan {
            actions: combined_actions,
        })
    }

    /// Parse and execute colon commands (:w, :q, etc.).
    fn execute_colon_command(&mut self, cmd: &str) -> Result<()> {
        let trimmed = cmd.trim();
        match trimmed {
            // :w to save changes (only confirms if something is being deleted)
            "w" | "write" => {
                let plan = self.collect_all_diffs()?;

                if plan.is_empty() {
                    self.set_status("No changes to save across any directory");
                    self.mode = Mode::Normal;
                } else if plan.has_deletions() && self.config.confirm_diff {
                    // Confirmation dialog only shown when deletions are present
                    self.pending_plan = Some(plan);
                    self.quit_after_confirm = false;
                    self.mode = Mode::ConfirmModal { scroll: 0 };
                } else {
                    // Directly save creates, renames, and moves
                    self.apply_plan(plan)?;
                    self.mode = Mode::Normal;
                }
            }

            // :w! to force save all directories without confirmation dialog
            "w!" => {
                let plan = self.collect_all_diffs()?;
                if plan.is_empty() {
                    self.set_status("No changes to save");
                } else {
                    self.apply_plan(plan)?;
                }
                self.mode = Mode::Normal;
            }

            // :q to quit
            "q" | "quit" => {
                let dirty_count = self.columns.iter().filter(|c| c.is_dirty()).count();
                if dirty_count > 0 {
                    self.set_status(format!(
                        "{} directories have unsaved edits! Use :w to save or :q! to force quit.",
                        dirty_count
                    ));
                    self.mode = Mode::Normal;
                } else {
                    self.should_quit = true;
                }
            }

            // :q! to force quit
            "q!" => {
                self.should_quit = true;
            }

            // :wq or :x
            "wq" | "x" => {
                let plan = self.collect_all_diffs()?;
                if plan.is_empty() {
                    self.should_quit = true;
                } else if plan.has_deletions() && self.config.confirm_diff {
                    self.pending_plan = Some(plan);
                    self.quit_after_confirm = true;
                    self.mode = Mode::ConfirmModal { scroll: 0 };
                } else {
                    self.apply_plan(plan)?;
                    self.should_quit = true;
                }
            }

            // :e to reload active directory
            "e" | "edit" => {
                let show_hidden = self.config.show_hidden;
                let natural_sort = self.config.natural_sort;
                self.active_buffer_mut()
                    .reload(show_hidden, natural_sort)?;
                self.set_status("Active directory reloaded from disk");
                self.mode = Mode::Normal;
                self.ensure_child_preview();
            }

            // :help or :h
            "h" | "help" => {
                self.mode = Mode::HelpModal { scroll: 0 };
            }

            // :set cursorline / :set nocursorline
            "set cursorline" | "set cul" => {
                self.config.cursorline = true;
                self.set_status("cursorline enabled");
                self.mode = Mode::Normal;
            }
            "set nocursorline" | "set nocul" => {
                self.config.cursorline = false;
                self.set_status("cursorline disabled");
                self.mode = Mode::Normal;
            }

            _ => {
                self.set_status(format!("Not an editor command: {}", trimmed));
                self.mode = Mode::Normal;
            }
        }

        Ok(())
    }

    /// Search Mode (/pattern).
    fn handle_search_mode(&mut self, key: KeyEvent, mut input: String) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                input.pop();
                self.mode = Mode::Search { input };
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.mode = Mode::Search { input };
            }
            KeyCode::Enter => {
                if !input.is_empty() {
                    self.search_query = Some(input.clone());
                    self.jump_to_next_match(&input);
                }
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    /// Jump to next filename matching search query in active buffer.
    fn jump_to_next_match(&mut self, query: &str) {
        let query_lower = query.to_lowercase();
        let total = self.active_buffer().lines.len();
        if total == 0 {
            return;
        }

        let start = (self.active_buffer().cursor_row + 1) % total;
        for i in 0..total {
            let idx = (start + i) % total;
            if self.active_buffer().lines[idx]
                .text
                .to_lowercase()
                .contains(&query_lower)
            {
                self.active_buffer_mut().cursor_row = idx;
                self.active_buffer_mut().clamp_cursor_normal();
                self.set_status(format!("Match: {}", self.active_buffer().lines[idx].text));
                self.ensure_child_preview();
                return;
            }
        }
        self.set_status(format!("Pattern not found: {}", query));
    }

    /// Jump to previous filename matching search query in active buffer.
    fn jump_to_prev_match(&mut self, query: &str) {
        let query_lower = query.to_lowercase();
        let total = self.active_buffer().lines.len();
        if total == 0 {
            return;
        }

        let current = self.active_buffer().cursor_row;
        for i in 1..=total {
            let idx = (current + total - (i % total)) % total;
            if self.active_buffer().lines[idx]
                .text
                .to_lowercase()
                .contains(&query_lower)
            {
                self.active_buffer_mut().cursor_row = idx;
                self.active_buffer_mut().clamp_cursor_normal();
                self.set_status(format!("Match: {}", self.active_buffer().lines[idx].text));
                self.ensure_child_preview();
                return;
            }
        }
        self.set_status(format!("Pattern not found: {}", query));
    }

    /// Diff Confirmation Modal event dispatching.
    fn handle_confirm_modal(&mut self, key: KeyEvent, mut scroll: usize) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                if let Some(plan) = self.pending_plan.take() {
                    self.apply_plan(plan)?;
                }
                if self.quit_after_confirm {
                    self.should_quit = true;
                } else {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                self.pending_plan = None;
                self.quit_after_confirm = false;
                self.set_status("Changes aborted");
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') => {
                if let Some(plan) = &self.pending_plan {
                    if scroll + 1 < plan.len() {
                        scroll += 1;
                        self.mode = Mode::ConfirmModal { scroll };
                    }
                }
            }
            KeyCode::Char('k') => {
                scroll = scroll.saturating_sub(1);
                self.mode = Mode::ConfirmModal { scroll };
            }
            _ => {}
        }
        Ok(())
    }

    /// Help Modal event dispatching.
    fn handle_help_modal(&mut self, key: KeyEvent, _scroll: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    // --- Helpers ---

    /// Commit the given plan to disk and reload all columns safely.
    fn apply_plan(&mut self, plan: FsPlan) -> Result<()> {
        let count = plan.len();
        let undo_batch = execute_plan(&plan, self.config.trash_deletes)?;
        self.undo_stack.push(undo_batch);

        self.reload_columns_after_fs_change();
        self.set_status(format!(
            "Successfully applied {} changes across directories",
            count
        ));
        Ok(())
    }

    /// Reload open directory columns after a disk transaction (`:w`, `u`, `Ctrl-r`),
    /// pruning any child columns whose directories were deleted or renamed.
    fn reload_columns_after_fs_change(&mut self) {
        let show_hidden = self.config.show_hidden;
        let natural_sort = self.config.natural_sort;

        let mut valid_len = 0;
        for col in &mut self.columns {
            if col.reload(show_hidden, natural_sort).is_ok() {
                valid_len += 1;
            } else {
                break;
            }
        }

        if valid_len == 0 && !self.columns.is_empty() {
            // Walk up to nearest existing parent if column 0 was deleted/renamed
            let mut probe = self.columns[0].current_dir.clone();
            while !probe.is_dir() {
                if let Some(p) = probe.parent() {
                    probe = p.to_path_buf();
                } else {
                    break;
                }
            }
            if let Ok(buf) = DirectoryBuffer::load(&probe, show_hidden, natural_sort) {
                self.columns = vec![buf];
            }
        } else if valid_len < self.columns.len() {
            self.columns.truncate(valid_len);
        }

        if !self.columns.is_empty() {
            self.active_col = self.active_col.min(self.columns.len() - 1);
        }

        // Refresh clipboard cut state so already-committed or reloaded items
        // don't carry stale EntryIds or nonexistent source paths.
        for item in &mut self.clipboard {
            item.line.entry_id = None;
            if let Some(ref src) = item.line.source_path {
                if !src.exists() {
                    item.is_cut = false;
                    item.line.source_path = None;
                }
            }
        }

        self.ensure_child_preview();
    }

    /// Safely paste clipboard items after (`after == true`, 'p') or before ('P') the cursor.
    fn paste_clipboard(&mut self, after: bool) {
        if self.clipboard.is_empty() {
            return;
        }

        self.active_buffer_mut().push_undo_snapshot();
        let items = self.clipboard.clone();

        for (idx, item) in items.iter().enumerate() {
            let mut pasted_line = item.line.clone();
            let buf_len = self.active_buffer().lines.len();
            let insert_idx = if after {
                if buf_len == 0 {
                    0
                } else {
                    (self.active_buffer().cursor_row + 1).min(buf_len)
                }
            } else {
                self.active_buffer().cursor_row.min(buf_len)
            };

            if item.is_cut {
                let source_exists = pasted_line
                    .source_path
                    .as_ref()
                    .map(|p| p.exists())
                    .unwrap_or(false);

                if !source_exists {
                    // Previously cut item no longer exists at source (already trashed/moved)
                    pasted_line.entry_id = None;
                    pasted_line.source_path = None;
                } else {
                    let is_same_dir = pasted_line
                        .source_path
                        .as_ref()
                        .and_then(|p| p.parent())
                        .map(|p| p == self.active_buffer().current_dir)
                        .unwrap_or(false);

                    // Check if the current buffer already contains a line referencing this source
                    let already_in_buf = pasted_line.source_path.as_ref().is_some_and(|src| {
                        self.active_buffer()
                            .lines
                            .iter()
                            .any(|l| l.source_path.as_ref() == Some(src))
                    });

                    if already_in_buf {
                        // File was already restored/present; treat extra paste as new entry
                        pasted_line.entry_id = None;
                        pasted_line.source_path = None;
                    } else if !is_same_dir {
                        pasted_line.entry_id = None;
                    } else if let Some(id) = pasted_line.entry_id {
                        // Verify EntryId still matches the same file in baseline after reloads
                        let matches_baseline = self
                            .active_buffer()
                            .baseline
                            .get(&id)
                            .map(|(name, _)| {
                                Some(self.active_buffer().current_dir.join(name))
                                    == pasted_line.source_path
                            })
                            .unwrap_or(false);
                        if !matches_baseline {
                            pasted_line.entry_id = None;
                        }
                    }
                }
            } else {
                pasted_line.entry_id = None;
                pasted_line.source_path = None;
            }

            self.active_buffer_mut().lines.insert(insert_idx, pasted_line);
            if after {
                self.active_buffer_mut().cursor_row = insert_idx;
            }

            // Consume cut flag so pasting a second time doesn't issue duplicate moves
            if let Some(orig) = self.clipboard.get_mut(idx) {
                orig.is_cut = false;
                orig.line.entry_id = None;
                orig.line.source_path = None;
            }
        }

        self.active_buffer_mut().clamp_cursor_normal();
        self.ensure_child_preview();
    }

    /// Open a file in the user's terminal $EDITOR.
    fn open_in_editor(&mut self, path: &Path, terminal: &mut Terminal) -> Result<()> {
        terminal.suspend()?;

        let parts: Vec<&str> = self.config.default_editor.split_whitespace().collect();
        let (bin, args) = parts.split_first().unwrap_or((&"nano", &[]));

        let mut cmd = Command::new(bin);
        cmd.args(args);
        cmd.arg(path);
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());

        let status = cmd.status();

        terminal.resume()?;

        match status {
            Ok(s) if !s.success() => {
                self.set_status(format!("Editor exited with status: {}", s));
            }
            Err(e) => {
                self.set_status(format!("Failed to run editor '{}': {}", bin, e));
            }
            _ => {
                let show_hidden = self.config.show_hidden;
                let natural_sort = self.config.natural_sort;
                if !self.active_buffer().is_dirty() {
                    let _ = self
                        .active_buffer_mut()
                        .reload(show_hidden, natural_sort);
                }
            }
        }

        Ok(())
    }

    /// Open item with system opener (e.g. xdg-open for 'gx').
    fn open_with_system_opener(&mut self) -> Result<()> {
        let active = &self.columns[self.active_col];
        if let Some(line) = active.current_line() {
            let target = active.current_dir.join(line.clean_name());
            let parts: Vec<&str> = self.config.opener.split_whitespace().collect();
            let (bin, args) = parts.split_first().unwrap_or((&"xdg-open", &[]));

            let res = Command::new(bin)
                .args(args)
                .arg(&target)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();

            match res {
                Ok(_) => self.set_status(format!("Opened with {}", bin)),
                Err(e) => self.set_status(format!("Opener error: {}", e)),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_keep_columns_open_when_cursor_on_file() {
        let temp_dir = std::env::temp_dir().join(format!("euler_col_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("subfolder")).unwrap();
        fs::create_dir_all(temp_dir.join("subfolder2")).unwrap();
        fs::write(temp_dir.join("a_file.txt"), "hello").unwrap();
        fs::write(temp_dir.join("subfolder/inner.txt"), "inner").unwrap();
        fs::write(temp_dir.join("subfolder2/other.txt"), "other").unwrap();

        let config = Config::default();
        let mut app = App::new(&temp_dir, config).unwrap();

        // Position cursor on 'subfolder'
        let subfolder_pos = app.columns[0]
            .lines
            .iter()
            .position(|l| l.clean_name() == "subfolder")
            .unwrap();
        app.columns[0].cursor_row = subfolder_pos;
        app.ensure_child_preview();

        // subfolder column should now be open (2 columns total)
        assert_eq!(app.columns.len(), 2);
        assert_eq!(
            app.columns[1].current_dir,
            temp_dir.join("subfolder")
        );

        // Move cursor onto a regular file ('a_file.txt')
        let file_pos = app.columns[0]
            .lines
            .iter()
            .position(|l| l.clean_name() == "a_file.txt")
            .unwrap();
        app.columns[0].cursor_row = file_pos;
        app.ensure_child_preview();

        // REQUIREMENT: Open column MUST NOT close when cursor moves to a file!
        assert_eq!(
            app.columns.len(),
            2,
            "Open column must remain viewable when cursor moves to a file"
        );
        assert_eq!(
            app.columns[1].current_dir,
            temp_dir.join("subfolder")
        );

        // Move cursor to another folder ('subfolder2')
        let subfolder2_pos = app.columns[0]
            .lines
            .iter()
            .position(|l| l.clean_name() == "subfolder2")
            .unwrap();
        app.columns[0].cursor_row = subfolder2_pos;
        app.ensure_child_preview();

        // REQUIREMENT: When cursor moves to a new folder, update the view!
        assert_eq!(app.columns.len(), 2);
        assert_eq!(
            app.columns[1].current_dir,
            temp_dir.join("subfolder2")
        );

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_move_file_operation_via_dd_and_paste() {
        let temp_dir = std::env::temp_dir().join(format!("euler_move_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let dir_a = temp_dir.join("folder_a");
        let dir_b = temp_dir.join("folder_b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();

        let file_path = dir_a.join("document.txt");
        fs::write(&file_path, "important data not to be lost").unwrap();

        let config = Config::default();
        let buf_a = DirectoryBuffer::load(&dir_a, config.show_hidden, config.natural_sort).unwrap();
        let buf_b = DirectoryBuffer::load(&dir_b, config.show_hidden, config.natural_sort).unwrap();

        let mut app = App {
            columns: vec![buf_a, buf_b],
            active_col: 0,
            config,
            mode: Mode::Normal,
            pending_plan: None,
            undo_stack: UndoStack::new(),
            clipboard: Vec::new(),
            search_query: None,
            status_msg: None,
            should_quit: false,
            quit_after_confirm: false,
        };

        // 1. In folder_a: delete with 'dd' (cut)
        let deleted = app.active_buffer_mut().delete_line().expect("Should delete document.txt");
        assert_eq!(deleted.clean_name(), "document.txt");
        assert_eq!(deleted.source_path, Some(file_path.clone()));

        app.clipboard = vec![ClipboardItem {
            line: deleted,
            is_cut: true,
        }];

        // 2. Navigate to folder_b (col 1) and paste with 'p'
        app.active_col = 1;
        let items = app.clipboard.clone();
        for item in &items {
            let mut pasted_line = item.line.clone();
            // Moved to a different directory: clear local entry_id
            pasted_line.entry_id = None;
            app.active_buffer_mut().lines.push(pasted_line);
        }

        // 3. Compute diff across all columns
        let plan = app.collect_all_diffs().unwrap();

        // CRITICAL: Must be a MOVE action, NOT Delete + Create!
        assert_eq!(plan.actions.len(), 1, "Plan should contain exactly 1 action (Move)");
        assert_eq!(
            plan.actions[0],
            FsAction::Move {
                from: dir_a.join("document.txt"),
                to: dir_b.join("document.txt"),
            }
        );
        assert!(!plan.has_deletions(), "Move plan must NOT have deletions");

        // 4. Apply the plan to disk
        app.apply_plan(plan).unwrap();

        // 5. Verify filesystem: file moved and content preserved!
        assert!(!dir_a.join("document.txt").exists(), "Source file should be moved away");
        assert!(dir_b.join("document.txt").exists(), "Destination file must exist");
        let content = fs::read_to_string(dir_b.join("document.txt")).unwrap();
        assert_eq!(content, "important data not to be lost");

        // 6. Test Undo: 'u' rolls back the move
        assert!(app.undo_stack.undo().unwrap());
        assert!(dir_a.join("document.txt").exists(), "File should be restored to folder_a");
        assert!(!dir_b.join("document.txt").exists(), "File should no longer be in folder_b");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_same_directory_dd_and_paste_is_not_delete() {
        let temp_dir = std::env::temp_dir().join(format!("euler_same_dir_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let file_path = temp_dir.join("file.txt");
        fs::write(&file_path, "same directory content").unwrap();

        let config = Config::default();
        let mut app = App::new(&temp_dir, config).unwrap();

        // 1. Delete line with 'dd'
        let deleted = app.active_buffer_mut().delete_line().unwrap();
        app.clipboard = vec![ClipboardItem {
            line: deleted,
            is_cut: true,
        }];

        // 2. Paste back in the SAME directory with 'p'
        let item = &app.clipboard[0];
        let pasted_line = item.line.clone();
        app.active_buffer_mut().lines.push(pasted_line);

        // 3. Compute diff: since source_path == target_path and entry_id is retained,
        // no filesystem action should be generated!
        let plan = app.collect_all_diffs().unwrap();
        assert!(
            plan.is_empty(),
            "Cutting and pasting in the same directory without rename should produce 0 changes"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_search_navigation_next_and_prev() {
        let temp_dir = std::env::temp_dir().join(format!("euler_search_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(temp_dir.join("alpha.txt"), "").unwrap();
        fs::write(temp_dir.join("beta.txt"), "").unwrap();
        fs::write(temp_dir.join("gamma.txt"), "").unwrap();
        fs::write(temp_dir.join("alphabet.txt"), "").unwrap();

        let config = Config::default();
        let mut app = App::new(&temp_dir, config).unwrap();
        // At start, cursor is at row 0 ("alpha.txt")
        assert_eq!(app.active_buffer().cursor_row, 0);

        // Next match 'n': advances to next match at row 1 ("alphabet.txt")
        app.jump_to_next_match("alpha");
        assert_eq!(app.active_buffer().cursor_row, 1);

        // Next match 'n' again: wraps around to row 0 ("alpha.txt")
        app.jump_to_next_match("alpha");
        assert_eq!(app.active_buffer().cursor_row, 0);

        // Previous match 'N': moves backward to row 1 ("alphabet.txt")
        app.jump_to_prev_match("alpha");
        assert_eq!(app.active_buffer().cursor_row, 1);

        // Previous match 'N' again: moves backward to row 0 ("alpha.txt")
        app.jump_to_prev_match("alpha");
        assert_eq!(app.active_buffer().cursor_row, 0);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_undo_delete_then_rename_and_paste_safety() {
        let temp_dir = std::env::temp_dir().join(format!("euler_undo_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("folder1")).unwrap();
        fs::write(temp_dir.join("a_file.txt"), "hello").unwrap();

        let mut config = Config::default();
        config.trash_deletes = false;
        let mut app = App::new(&temp_dir, config).unwrap();

        // 1. Delete 'a_file.txt' in buffer (dd)
        app.active_buffer_mut().push_undo_snapshot();
        let deleted = app.active_buffer_mut().delete_line().unwrap();
        assert_eq!(deleted.clean_name(), "a_file.txt");
        app.clipboard = vec![ClipboardItem {
            line: deleted,
            is_cut: true,
        }];
        app.ensure_child_preview();

        // 'folder1' preview should now be open as column 1
        assert_eq!(app.columns.len(), 2);

        // 2. Rename 'folder1' to 'folder2' in buffer
        app.active_buffer_mut().push_undo_snapshot();
        app.active_buffer_mut().lines[0].text = "folder2".to_string();

        // 3. Pressing undo ('u') first undoes ONLY the rename, NOT the delete!
        assert!(app.active_buffer_mut().undo_edit());
        assert_eq!(app.active_buffer().lines.len(), 1);
        assert_eq!(app.active_buffer().lines[0].clean_name(), "folder1");

        // 4. Pressing undo ('u') a second time undoes the delete!
        assert!(app.active_buffer_mut().undo_edit());
        assert_eq!(app.active_buffer().lines.len(), 2);

        // 5. Redo both edits, delete remaining line so buffer is empty, and paste without panicking!
        assert!(app.active_buffer_mut().redo_edit());
        app.active_buffer_mut().push_undo_snapshot();
        let _ = app.active_buffer_mut().delete_line();
        assert!(app.active_buffer().lines.is_empty());

        // Pasting into an empty buffer with 'p' or 'P' must never panic
        app.paste_clipboard(true);
        assert_eq!(app.active_buffer().lines.len(), 1);
        assert_eq!(app.active_buffer().lines[0].clean_name(), "a_file.txt");

        // 6. Test renaming an open child column folder + apply_plan reloads safely
        app.reload_columns_after_fs_change();
        app.active_buffer_mut().cursor_row = 1; // folder1
        app.ensure_child_preview();
        assert_eq!(app.columns.len(), 2);
        app.active_buffer_mut().lines[1].text = "folder_renamed".to_string();
        let plan = app.collect_all_diffs().unwrap();
        app.apply_plan(plan).unwrap();
        assert!(temp_dir.join("folder_renamed").exists());

        // Undo filesystem rename cleanly restores folder1 and updates columns
        assert!(app.undo_stack.undo().unwrap());
        app.reload_columns_after_fs_change();
        assert!(temp_dir.join("folder1").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
