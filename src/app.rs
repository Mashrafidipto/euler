//! Application controller: owns global state and orchestrates user input.

use crate::buffer::DirectoryBuffer;
use crate::config::Config;
use crate::engine::{compute_diff, execute_plan, FsPlan, UndoStack};
use crate::input::Mode;
use crate::ui::Terminal;
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct App {
    pub buffer: DirectoryBuffer,
    pub config: Config,
    pub mode: Mode,
    pub pending_plan: Option<FsPlan>,
    pub undo_stack: UndoStack,
    pub clipboard: Vec<String>,
    pub search_query: Option<String>,
    pub status_msg: Option<(String, Instant)>,
    pub should_quit: bool,
}

impl App {
    /// Initialize Euler on a target directory.
    pub fn new(path: &Path, config: Config) -> Result<Self> {
        let buffer = DirectoryBuffer::load(path, config.show_hidden, config.natural_sort)?;
        Ok(Self {
            buffer,
            config,
            mode: Mode::Normal,
            pending_plan: None,
            undo_stack: UndoStack::new(),
            clipboard: Vec::new(),
            search_query: None,
            status_msg: None,
            should_quit: false,
        })
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

    /// Main application loop with event-driven rendering (0% idle CPU and zero tearing).
    pub fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        let mut needs_render = true;

        while !self.should_quit {
            let status_msg = self.current_status();

            // Only redraw when state has actually changed or on window resize
            if needs_render {
                terminal.render(
                    &mut self.buffer,
                    &self.mode,
                    self.pending_plan.as_ref(),
                    status_msg.as_deref(),
                )?;
                needs_render = false;
            }

            // If a status message is active, poll briefly so it can expire; otherwise wait for input
            let poll_duration = if self.status_msg.is_some() {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(60)
            };

            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key, terminal)?;
                            needs_render = true;
                        }
                    }
                    Event::Resize(_, _) => {
                        needs_render = true;
                    }
                    _ => {}
                }
            } else if self.status_msg.is_some() {
                // Status message might have expired, trigger re-render
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
            Mode::DirtyNavPrompt { target, is_parent } => {
                let t = target.clone();
                let p = *is_parent;
                self.handle_dirty_prompt(key, t, p)?;
            }
        }

        Ok(())
    }

    // --- Mode Handlers ---

    /// Normal Mode event dispatching.
    fn handle_normal_mode(&mut self, key: KeyEvent, terminal: &mut Terminal) -> Result<()> {
        match key.code {
            // Directory Traversal: 'h' moves to parent directory (..)
            KeyCode::Char('h') => {
                let parent = self.buffer.current_dir.parent().map(|p| p.to_path_buf());
                if let Some(parent_path) = parent {
                    if self.buffer.is_dirty() {
                        self.mode = Mode::DirtyNavPrompt {
                            target: parent_path,
                            is_parent: true,
                        };
                    } else {
                        self.navigate_to(parent_path)?;
                    }
                } else {
                    self.set_status("Already at root directory.");
                }
            }

            // Directory Traversal: 'l' or Enter enters directory / opens file
            KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(line) = self.buffer.current_line() {
                    let target_path = self.buffer.current_dir.join(line.clean_name());
                    if line.is_dir() {
                        if self.buffer.is_dirty() {
                            self.mode = Mode::DirtyNavPrompt {
                                target: target_path,
                                is_parent: false,
                            };
                        } else {
                            self.navigate_to(target_path)?;
                        }
                    } else {
                        // Open file in user's $EDITOR
                        self.open_in_editor(&target_path, terminal)?;
                    }
                }
            }

            // Hyphen is disabled by user specification
            KeyCode::Char('-') => {
                // Do nothing
            }

            // Open with xdg-open: 'gx'
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::NONE) => {
                // Single 'x' deletes char under cursor
                self.buffer.delete_char_under();
            }

            // 'g' prefix chords
            KeyCode::Char('g') => {
                // Wait for next key chord
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press {
                        match next.code {
                            // 'gg' jumps to top of buffer
                            KeyCode::Char('g') => {
                                self.buffer.cursor_row = 0;
                                self.buffer.clamp_cursor_normal();
                            }
                            // 'g.' toggles hidden files
                            KeyCode::Char('.') => {
                                self.config.show_hidden = !self.config.show_hidden;
                                self.buffer.reload(self.config.show_hidden, self.config.natural_sort)?;
                                let state = if self.config.show_hidden { "visible" } else { "hidden" };
                                self.set_status(format!("Dotfiles are now {}", state));
                            }
                            // 'gx' opens target with xdg-open
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
                if !self.buffer.lines.is_empty() {
                    self.buffer.cursor_row = (self.buffer.cursor_row + 1).min(self.buffer.lines.len() - 1);
                    self.buffer.clamp_cursor_normal();
                }
            }
            KeyCode::Char('k') => {
                self.buffer.cursor_row = self.buffer.cursor_row.saturating_sub(1);
                self.buffer.clamp_cursor_normal();
            }

            // Horizontal Motions: 'w', 'b', '0', '$'
            KeyCode::Char('w') => {
                if let Some(line) = self.buffer.current_line() {
                    let chars: Vec<char> = line.text.chars().collect();
                    let mut col = self.buffer.cursor_col;
                    // Jump past current word
                    while col < chars.len() && !chars[col].is_whitespace() {
                        col += 1;
                    }
                    // Jump past whitespace to next word
                    while col < chars.len() && chars[col].is_whitespace() {
                        col += 1;
                    }
                    self.buffer.cursor_col = col;
                    self.buffer.clamp_cursor_normal();
                }
            }
            KeyCode::Char('b') => {
                if let Some(line) = self.buffer.current_line() {
                    let chars: Vec<char> = line.text.chars().collect();
                    let mut col = self.buffer.cursor_col;
                    if col > 0 {
                        col -= 1;
                        while col > 0 && chars[col].is_whitespace() {
                            col -= 1;
                        }
                        while col > 0 && !chars[col - 1].is_whitespace() {
                            col -= 1;
                        }
                    }
                    self.buffer.cursor_col = col;
                }
            }
            KeyCode::Char('0') => {
                self.buffer.cursor_col = 0;
            }
            KeyCode::Char('$') => {
                if let Some(line) = self.buffer.current_line() {
                    let len = line.text.chars().count();
                    self.buffer.cursor_col = len.saturating_sub(1);
                }
            }
            KeyCode::Char('G') => {
                if !self.buffer.lines.is_empty() {
                    self.buffer.cursor_row = self.buffer.lines.len() - 1;
                    self.buffer.clamp_cursor_normal();
                }
            }

            // Scrolling: Ctrl-d / Ctrl-u
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let half = 10;
                if !self.buffer.lines.is_empty() {
                    self.buffer.cursor_row = (self.buffer.cursor_row + half).min(self.buffer.lines.len() - 1);
                    self.buffer.clamp_cursor_normal();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let half = 10;
                self.buffer.cursor_row = self.buffer.cursor_row.saturating_sub(half);
                self.buffer.clamp_cursor_normal();
            }

            // Entering Insert Mode: 'i', 'a', 'I', 'A', 'o', 'O'
            KeyCode::Char('i') => {
                self.mode = Mode::Insert;
            }
            KeyCode::Char('a') => {
                if let Some(line) = self.buffer.current_line() {
                    let len = line.text.chars().count();
                    self.buffer.cursor_col = (self.buffer.cursor_col + 1).min(len);
                }
                self.mode = Mode::Insert;
            }
            KeyCode::Char('I') => {
                self.buffer.cursor_col = 0;
                self.mode = Mode::Insert;
            }
            KeyCode::Char('A') => {
                if let Some(line) = self.buffer.current_line() {
                    self.buffer.cursor_col = line.text.chars().count();
                }
                self.mode = Mode::Insert;
            }
            KeyCode::Char('o') => {
                self.buffer.insert_line_below();
                self.mode = Mode::Insert;
            }
            KeyCode::Char('O') => {
                self.buffer.insert_line_above();
                self.mode = Mode::Insert;
            }

            // Change operations: 'c' prefix
            KeyCode::Char('c') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press {
                        match next.code {
                            KeyCode::Char('w') => {
                                // Change word
                                let cursor_col = self.buffer.cursor_col;
                                if let Some(line) = self.buffer.current_line_mut() {
                                    let chars: Vec<char> = line.text.chars().collect();
                                    let mut end = cursor_col;
                                    while end < chars.len() && !chars[end].is_whitespace() {
                                        end += 1;
                                    }
                                    let mut new_chars = chars[..cursor_col].to_vec();
                                    new_chars.extend_from_slice(&chars[end..]);
                                    line.text = new_chars.into_iter().collect();
                                }
                                self.mode = Mode::Insert;
                            }
                            KeyCode::Char('c') => {
                                // Clear whole line
                                if let Some(line) = self.buffer.current_line_mut() {
                                    line.text.clear();
                                }
                                self.buffer.cursor_col = 0;
                                self.mode = Mode::Insert;
                            }
                            _ => {}
                        }
                    }
                }
            }
            KeyCode::Char('C') => {
                self.buffer.delete_to_end_of_line();
                self.mode = Mode::Insert;
            }

            // Deletions: 'd' prefix
            KeyCode::Char('d') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press && next.code == KeyCode::Char('d') {
                        if let Some(deleted) = self.buffer.delete_line() {
                            self.clipboard = vec![deleted.text];
                        }
                    }
                }
            }
            KeyCode::Char('D') => {
                self.buffer.delete_to_end_of_line();
            }

            // Yank & Paste: 'y', 'p', 'P'
            KeyCode::Char('y') => {
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press && next.code == KeyCode::Char('y') {
                        if let Some(line) = self.buffer.current_line() {
                            self.clipboard = vec![line.text.clone()];
                            self.set_status("Line yanked");
                        }
                    }
                }
            }
            KeyCode::Char('p') => {
                if !self.clipboard.is_empty() {
                    for text in &self.clipboard {
                        let insert_idx = self.buffer.cursor_row + 1;
                        self.buffer.lines.insert(
                            insert_idx,
                            crate::buffer::BufferLine::new_with_text(text.clone()),
                        );
                        self.buffer.cursor_row = insert_idx;
                    }
                    self.buffer.clamp_cursor_normal();
                }
            }
            KeyCode::Char('P') => {
                if !self.clipboard.is_empty() {
                    for text in &self.clipboard {
                        let insert_idx = self.buffer.cursor_row;
                        self.buffer.lines.insert(
                            insert_idx,
                            crate::buffer::BufferLine::new_with_text(text.clone()),
                        );
                    }
                    self.buffer.clamp_cursor_normal();
                }
            }

            // Visual Line Mode: 'V'
            KeyCode::Char('V') => {
                self.mode = Mode::VisualLine {
                    start_row: self.buffer.cursor_row,
                };
            }

            // Undo / Redo: 'u', 'Ctrl-r'
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::NONE) => {
                // Check filesystem undo stack
                if self.undo_stack.undo()? {
                    self.buffer.reload(self.config.show_hidden, self.config.natural_sort)?;
                    self.set_status("Undid last filesystem change");
                } else {
                    self.set_status("Already at oldest change");
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.undo_stack.redo()? {
                    self.buffer.reload(self.config.show_hidden, self.config.natural_sort)?;
                    self.set_status("Redid filesystem change");
                } else {
                    self.set_status("Already at newest change");
                }
            }
            KeyCode::Char('r') => {
                // Replace single character
                if let Ok(Event::Key(next)) = event::read() {
                    if next.kind == KeyEventKind::Press {
                        if let KeyCode::Char(ch) = next.code {
                            self.buffer.delete_char_under();
                            self.buffer.insert_char(ch);
                            self.buffer.clamp_cursor_normal();
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

            _ => {}
        }

        Ok(())
    }

    /// Insert Mode event dispatching.
    fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.buffer.clamp_cursor_normal();
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.buffer.delete_char_before();
            }
            KeyCode::Delete => {
                self.buffer.delete_char_under();
            }
            KeyCode::Enter => {
                self.buffer.insert_line_below();
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.buffer.delete_word_before();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.buffer.clear_to_start();
            }
            KeyCode::Char(c) => {
                self.buffer.insert_char(c);
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
                if !self.buffer.lines.is_empty() {
                    self.buffer.cursor_row = (self.buffer.cursor_row + 1).min(self.buffer.lines.len() - 1);
                }
            }
            KeyCode::Char('k') => {
                self.buffer.cursor_row = self.buffer.cursor_row.saturating_sub(1);
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let (low, high) = if start_row <= self.buffer.cursor_row {
                    (start_row, self.buffer.cursor_row)
                } else {
                    (self.buffer.cursor_row, start_row)
                };

                let mut deleted_texts = Vec::new();
                for _ in low..=high {
                    if low < self.buffer.lines.len() {
                        let removed = self.buffer.lines.remove(low);
                        deleted_texts.push(removed.text);
                    }
                }
                self.clipboard = deleted_texts;
                self.buffer.cursor_row = low.min(self.buffer.lines.len().saturating_sub(1));
                self.buffer.clamp_cursor_normal();
                self.mode = Mode::Normal;
            }
            KeyCode::Char('y') => {
                let (low, high) = if start_row <= self.buffer.cursor_row {
                    (start_row, self.buffer.cursor_row)
                } else {
                    (self.buffer.cursor_row, start_row)
                };

                self.clipboard = self.buffer.lines[low..=high]
                    .iter()
                    .map(|l| l.text.clone())
                    .collect();
                self.set_status(format!("{} lines yanked", self.clipboard.len()));
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

    /// Parse and execute colon commands (:w, :q, etc.).
    fn execute_colon_command(&mut self, cmd: &str) -> Result<()> {
        let trimmed = cmd.trim();
        match trimmed {
            // :w to view diff and save changes
            "w" | "write" => {
                let plan = compute_diff(
                    &self.buffer.current_dir,
                    &self.buffer.baseline,
                    &self.buffer.lines,
                )?;

                if plan.is_empty() {
                    self.set_status("No changes to save");
                    self.mode = Mode::Normal;
                } else if self.config.confirm_diff {
                    self.pending_plan = Some(plan);
                    self.mode = Mode::ConfirmModal { scroll: 0 };
                } else {
                    self.apply_plan(plan)?;
                    self.mode = Mode::Normal;
                }
            }

            // :w! to force save without confirmation dialog
            "w!" => {
                let plan = compute_diff(
                    &self.buffer.current_dir,
                    &self.buffer.baseline,
                    &self.buffer.lines,
                )?;
                if plan.is_empty() {
                    self.set_status("No changes to save");
                } else {
                    self.apply_plan(plan)?;
                }
                self.mode = Mode::Normal;
            }

            // :q to quit
            "q" | "quit" => {
                if self.buffer.is_dirty() {
                    self.set_status("Unsaved buffer edits! Use :w to save or :q! to force quit.");
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
                let plan = compute_diff(
                    &self.buffer.current_dir,
                    &self.buffer.baseline,
                    &self.buffer.lines,
                )?;
                if !plan.is_empty() {
                    self.apply_plan(plan)?;
                }
                self.should_quit = true;
            }

            // :e to reload directory
            "e" | "edit" => {
                self.buffer.reload(self.config.show_hidden, self.config.natural_sort)?;
                self.set_status("Directory reloaded from disk");
                self.mode = Mode::Normal;
            }

            // :help or :h
            "h" | "help" => {
                self.mode = Mode::HelpModal { scroll: 0 };
            }

            // Unknown
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

    /// Jump to next filename matching search query.
    fn jump_to_next_match(&mut self, query: &str) {
        let query_lower = query.to_lowercase();
        let total = self.buffer.lines.len();
        if total == 0 {
            return;
        }

        let start = (self.buffer.cursor_row + 1) % total;
        for i in 0..total {
            let idx = (start + i) % total;
            if self.buffer.lines[idx].text.to_lowercase().contains(&query_lower) {
                self.buffer.cursor_row = idx;
                self.buffer.clamp_cursor_normal();
                self.set_status(format!("Match: {}", self.buffer.lines[idx].text));
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
                self.mode = Mode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                self.pending_plan = None;
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

    /// Dirty Navigation Guard prompt.
    fn handle_dirty_prompt(&mut self, key: KeyEvent, target: PathBuf, _is_parent: bool) -> Result<()> {
        match key.code {
            // 'w' to review and save before navigating
            KeyCode::Char('w') => {
                let plan = compute_diff(
                    &self.buffer.current_dir,
                    &self.buffer.baseline,
                    &self.buffer.lines,
                )?;
                if !plan.is_empty() {
                    self.apply_plan(plan)?;
                }
                self.navigate_to(target)?;
                self.mode = Mode::Normal;
            }
            // 'd' to discard and navigate
            KeyCode::Char('d') => {
                self.navigate_to(target)?;
                self.mode = Mode::Normal;
            }
            // Esc to cancel navigation
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    // --- Helpers ---

    /// Commit the given plan to disk and reload the buffer.
    fn apply_plan(&mut self, plan: FsPlan) -> Result<()> {
        let count = plan.len();
        let undo_batch = execute_plan(&plan, self.config.trash_deletes)?;
        self.undo_stack.push(undo_batch);
        self.buffer.reload(self.config.show_hidden, self.config.natural_sort)?;
        self.set_status(format!("Successfully applied {} changes", count));
        Ok(())
    }

    /// Switch active directory buffer.
    fn navigate_to(&mut self, path: PathBuf) -> Result<()> {
        self.buffer = DirectoryBuffer::load(&path, self.config.show_hidden, self.config.natural_sort)?;
        Ok(())
    }

    /// Open a file in the user's terminal $EDITOR.
    fn open_in_editor(&mut self, path: &Path, terminal: &mut Terminal) -> Result<()> {
        // Temporarily leave alternate screen
        terminal.restore()?;

        let status = Command::new(&self.config.default_editor)
            .arg(path)
            .status()
            .with_context(|| format!("Failed to launch editor: {}", self.config.default_editor));

        // Re-initialize terminal
        *terminal = Terminal::init()?;

        if let Err(e) = status {
            self.set_status(format!("Editor error: {}", e));
        }

        Ok(())
    }

    /// Open item with system opener (e.g. xdg-open for 'gx').
    fn open_with_system_opener(&mut self) -> Result<()> {
        if let Some(line) = self.buffer.current_line() {
            let target = self.buffer.current_dir.join(line.clean_name());
            let res = Command::new(&self.config.opener)
                .arg(&target)
                .spawn();

            match res {
                Ok(_) => self.set_status(format!("Opened with {}", self.config.opener)),
                Err(e) => self.set_status(format!("Opener error: {}", e)),
            }
        }
        Ok(())
    }
}
