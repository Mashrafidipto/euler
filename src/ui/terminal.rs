//! Terminal lifecycle and multi-column buffered rendering engine.

use super::theme::Theme;
use crate::buffer::DirectoryBuffer;
use crate::engine::FsPlan;
use crate::input::Mode;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::io::{stdout, BufWriter, Stdout, Write};

pub struct Terminal {
    stdout: BufWriter<Stdout>,
    pub theme: Theme,
    is_active: bool,
    pub cursorline: bool,
}

impl Terminal {
    /// Initialize the terminal into raw mode, alternate screen, and buffered I/O.
    pub fn init() -> Result<Self> {
        enable_raw_mode()?;
        let mut raw_stdout = stdout();
        execute!(raw_stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
        raw_stdout.flush()?;

        let stdout = BufWriter::with_capacity(64 * 1024, raw_stdout);

        Ok(Self {
            stdout,
            theme: Theme::default(),
            is_active: true,
            cursorline: true,
        })
    }

    /// Suspend terminal raw mode and alternate screen to yield control to an external CLI tool.
    pub fn suspend(&mut self) -> Result<()> {
        if self.is_active {
            self.is_active = false;
            let _ = disable_raw_mode();
            let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
            let _ = self.stdout.flush();
        }
        Ok(())
    }

    /// Resume terminal raw mode and alternate screen after an external tool exits.
    pub fn resume(&mut self) -> Result<()> {
        if !self.is_active {
            enable_raw_mode()?;
            execute!(self.stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
            self.stdout.flush()?;
            self.is_active = true;
        }
        Ok(())
    }

    /// Restore terminal to normal settings on exit.
    pub fn restore(&mut self) -> Result<()> {
        if self.is_active {
            self.is_active = false;
            let _ = disable_raw_mode();
            let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
            let _ = self.stdout.flush();
        }
        Ok(())
    }

    /// Get current terminal dimensions (width, height).
    pub fn size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    /// Render multi-column view (up to 4 directories in a row like mini.files).
    pub fn render(
        &mut self,
        columns: &mut [DirectoryBuffer],
        active_col: usize,
        view_offset: usize,
        mode: &Mode,
        pending_plan: Option<&FsPlan>,
        status_msg: Option<&str>,
    ) -> Result<()> {
        let (width, height) = self.size();
        if width < 30 || height < 6 || columns.is_empty() {
            return Ok(());
        }

        let header_row = 0;
        let content_start_row = 1;
        let content_height = height.saturating_sub(3) as usize;
        let status_row = height.saturating_sub(2);
        let cmd_row = height.saturating_sub(1);

        let active_buf = &columns[active_col];
        let active_dir = active_buf.current_dir.clone();
        let total_dirty: usize = columns.iter().filter(|c| c.is_dirty()).count();

        // 1. Draw Global Top Header
        self.draw_global_header(&active_dir, active_col, columns.len(), total_dirty, width, header_row)?;

        // 2. Draw Multi-Column Panes (Up to 4 directories side-by-side)
        let active_col_x = self.draw_columns(
            columns,
            active_col,
            view_offset,
            mode,
            width,
            content_start_row,
            content_height,
        )?;

        // 3. Draw Status Bar
        self.draw_status_bar(&columns[active_col], active_col, columns.len(), mode, width, status_row)?;

        // 4. Draw Command Line / Message Bar
        self.draw_command_line(mode, status_msg, width, cmd_row)?;

        // 5. Draw Overlays / Modals (if active)
        match mode {
            Mode::ConfirmModal { scroll } => {
                if let Some(plan) = pending_plan {
                    self.draw_confirm_modal(plan, *scroll, width, height)?;
                }
            }
            Mode::HelpModal { scroll } => {
                self.draw_help_modal(*scroll, width, height)?;
            }
            _ => {}
        }

        // Hardware cursor positioning for Insert / Command / Search modes
        match mode {
            Mode::Insert => {
                let active = &columns[active_col];
                let line_num_width = 4;
                let prefix_width = 3;
                let cursor_x = active_col_x + line_num_width + prefix_width + active.cursor_col as u16;
                let cursor_y = content_start_row + 1 + (active.cursor_row.saturating_sub(active.scroll_offset)) as u16;
                queue!(self.stdout, Show, MoveTo(cursor_x, cursor_y))?;
            }
            Mode::Command { input } => {
                queue!(self.stdout, Show, MoveTo(1 + input.chars().count() as u16, cmd_row))?;
            }
            Mode::Search { input } => {
                queue!(self.stdout, Show, MoveTo(1 + input.chars().count() as u16, cmd_row))?;
            }
            _ => {
                queue!(self.stdout, Hide)?;
            }
        }

        self.stdout.flush()?;
        Ok(())
    }

    /// Draw global header.
    fn draw_global_header(
        &mut self,
        active_dir: &std::path::Path,
        active_col: usize,
        total_cols: usize,
        total_dirty: usize,
        width: u16,
        row: u16,
    ) -> Result<()> {
        queue!(
            self.stdout,
            MoveTo(0, row),
            SetBackgroundColor(Color::DarkBlue),
            SetForegroundColor(Color::White)
        )?;

        let dirty_str = if total_dirty > 0 {
            format!(" [{} modified]", total_dirty)
        } else {
            String::new()
        };

        let title = format!(" 📁 {}{}", active_dir.display(), dirty_str);
        let pane_info = format!("Pane {}/{} ", active_col + 1, total_cols);

        let title_chars = title.chars().count();
        let pane_chars = pane_info.chars().count();
        let space = (width as usize).saturating_sub(title_chars + pane_chars);
        let full = format!("{}{}{}", title, " ".repeat(space), pane_info);

        queue!(self.stdout, Print(safe_truncate(&full, width as usize)), ResetColor)?;
        Ok(())
    }

    /// Render directory panes side-by-side in columns (up to 4 columns in a row).
    /// Returns the X screen coordinate of the active column.
    #[allow(clippy::too_many_arguments)]
    fn draw_columns(
        &mut self,
        columns: &mut [DirectoryBuffer],
        active_col: usize,
        view_offset: usize,
        mode: &Mode,
        width: u16,
        start_row: u16,
        content_height: usize,
    ) -> Result<u16> {
        let max_visible_cols = 4.min(columns.len());
        let max_start = columns.len().saturating_sub(max_visible_cols);
        let visible_start = view_offset.min(max_start);
        let visible_end = (visible_start + max_visible_cols).min(columns.len());
        let num_visible = visible_end - visible_start;

        let total_sep_width = (num_visible as u16).saturating_sub(1);
        let col_width = (width.saturating_sub(total_sep_width)) / (num_visible as u16);

        let mut active_screen_x = 0;

        for (v_idx, col_idx) in (visible_start..visible_end).enumerate() {
            let col_x = v_idx as u16 * (col_width + 1);
            let is_active = col_idx == active_col;

            if is_active {
                active_screen_x = col_x;
            }

            let buf = &mut columns[col_idx];

            // Adjust vertical scroll offset for this buffer
            let list_height = content_height.saturating_sub(1); // 1 row for pane header
            if buf.cursor_row < buf.scroll_offset {
                buf.scroll_offset = buf.cursor_row;
            } else if buf.cursor_row >= buf.scroll_offset + list_height {
                buf.scroll_offset = buf.cursor_row + 1 - list_height;
            }

            // 1. Draw Column Title Row
            queue!(self.stdout, MoveTo(col_x, start_row))?;
            let folder_name = buf
                .current_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());

            let dirty_mark = if buf.is_dirty() { " [+]" } else { "" };
            let title = format!(" 📂 {}{}", folder_name, dirty_mark);

            if is_active {
                queue!(
                    self.stdout,
                    SetBackgroundColor(Color::DarkCyan),
                    SetForegroundColor(Color::White)
                )?;
            } else {
                queue!(
                    self.stdout,
                    SetBackgroundColor(Color::DarkGrey),
                    SetForegroundColor(Color::White)
                )?;
            }
            let padded_title = safe_pad_truncate(&title, col_width as usize);
            queue!(self.stdout, Print(&padded_title), ResetColor)?;

            // 2. Draw Column Buffer Lines
            let visual_range = match mode {
                Mode::VisualLine { start_row: start } if is_active => {
                    let (low, high) = if *start <= buf.cursor_row {
                        (*start, buf.cursor_row)
                    } else {
                        (buf.cursor_row, *start)
                    };
                    Some(low..=high)
                }
                _ => None,
            };

            for row_idx in 0..list_height {
                let line_idx = buf.scroll_offset + row_idx;
                let screen_y = start_row + 1 + row_idx as u16;

                queue!(self.stdout, MoveTo(col_x, screen_y))?;

                if line_idx >= buf.lines.len() {
                    // Empty row, pad with spaces
                    queue!(
                        self.stdout,
                        SetForegroundColor(Color::DarkGrey),
                        Print("~"),
                        Print(" ".repeat((col_width as usize).saturating_sub(1))),
                        ResetColor
                    )?;
                } else {
                    let line = &buf.lines[line_idx];
                    let is_cursor_row = line_idx == buf.cursor_row;
                    let is_visual_selected = visual_range.as_ref().map(|r| r.contains(&line_idx)).unwrap_or(false);
                    let apply_cursorline = is_active && is_cursor_row && self.cursorline && !is_visual_selected;

                    let line_num_str = format!("{:>3} ", line_idx + 1);
                    if apply_cursorline {
                        queue!(
                            self.stdout,
                            SetBackgroundColor(self.theme.cursor_line_bg),
                            SetForegroundColor(Color::Yellow),
                            SetAttribute(Attribute::Bold),
                            Print(&line_num_str)
                        )?;
                    } else {
                        queue!(
                            self.stdout,
                            SetForegroundColor(self.theme.line_number_color),
                            Print(&line_num_str),
                            ResetColor
                        )?;
                    }

                    let icon = if line.is_dir() {
                        "📁 "
                    } else if line.is_symlink() {
                        "🔗 "
                    } else {
                        "📄 "
                    };

                    let item_color = if line.is_dir() {
                        self.theme.dir_color
                    } else if line.is_symlink() {
                        self.theme.symlink_color
                    } else if line.text.starts_with('.') {
                        self.theme.hidden_color
                    } else {
                        self.theme.file_color
                    };

                    if apply_cursorline {
                        queue!(
                            self.stdout,
                            SetBackgroundColor(self.theme.cursor_line_bg),
                            SetForegroundColor(item_color),
                            Print(icon)
                        )?;
                    } else {
                        queue!(self.stdout, SetForegroundColor(item_color), Print(icon))?;
                    }

                    let mut used_cols = 4 + 3;
                    let max_text_cols = (col_width as usize).saturating_sub(used_cols);

                    if is_active && is_visual_selected {
                        let text = safe_pad_truncate(&line.text, max_text_cols);
                        queue!(
                            self.stdout,
                            SetBackgroundColor(Color::DarkMagenta),
                            SetForegroundColor(Color::White),
                            Print(&text),
                            ResetColor
                        )?;
                    } else if apply_cursorline {
                        let chars: Vec<char> = line.text.chars().collect();
                        if chars.is_empty() {
                            if *mode == Mode::Normal {
                                queue!(
                                    self.stdout,
                                    SetBackgroundColor(Color::Yellow),
                                    SetForegroundColor(Color::Black),
                                    SetAttribute(Attribute::Bold),
                                    Print(" "),
                                    SetBackgroundColor(self.theme.cursor_line_bg)
                                )?;
                                used_cols += 1;
                            }
                        } else {
                            for (c_idx, &ch) in chars.iter().take(max_text_cols).enumerate() {
                                if *mode == Mode::Normal && c_idx == buf.cursor_col {
                                    queue!(
                                        self.stdout,
                                        SetBackgroundColor(Color::Yellow),
                                        SetForegroundColor(Color::Black),
                                        SetAttribute(Attribute::Bold),
                                        Print(ch),
                                        SetBackgroundColor(self.theme.cursor_line_bg)
                                    )?;
                                } else {
                                    queue!(
                                        self.stdout,
                                        SetBackgroundColor(self.theme.cursor_line_bg),
                                        SetForegroundColor(item_color),
                                        SetAttribute(Attribute::Bold),
                                        Print(ch)
                                    )?;
                                }
                            }
                            used_cols += chars.len().min(max_text_cols);
                        }

                        let pad = (col_width as usize).saturating_sub(used_cols);
                        if pad > 0 {
                            queue!(
                                self.stdout,
                                SetBackgroundColor(self.theme.cursor_line_bg),
                                Print(" ".repeat(pad)),
                                SetAttribute(Attribute::Reset),
                                ResetColor
                            )?;
                        } else {
                            queue!(self.stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                        }
                    } else if is_active && is_cursor_row && *mode == Mode::Normal {
                        // Normal mode cursor on active column
                        let chars: Vec<char> = line.text.chars().collect();
                        if chars.is_empty() {
                            queue!(
                                self.stdout,
                                SetBackgroundColor(Color::Yellow),
                                SetForegroundColor(Color::Black),
                                SetAttribute(Attribute::Bold),
                                Print(" "),
                                SetAttribute(Attribute::Reset),
                                ResetColor
                            )?;
                            used_cols += 1;
                        } else {
                            for (c_idx, &ch) in chars.iter().take(max_text_cols).enumerate() {
                                if c_idx == buf.cursor_col {
                                    queue!(
                                        self.stdout,
                                        SetBackgroundColor(Color::Yellow),
                                        SetForegroundColor(Color::Black),
                                        SetAttribute(Attribute::Bold),
                                        Print(ch),
                                        SetAttribute(Attribute::Reset),
                                        ResetColor
                                    )?;
                                } else {
                                    queue!(self.stdout, SetForegroundColor(item_color), Print(ch))?;
                                }
                            }
                            used_cols += chars.len().min(max_text_cols);
                        }
                        let pad = (col_width as usize).saturating_sub(used_cols);
                        if pad > 0 {
                            queue!(self.stdout, ResetColor, Print(" ".repeat(pad)))?;
                        }
                    } else if !is_active && is_cursor_row {
                        // Simple, subtle indicator in inactive column to show opened folder breadcrumb
                        queue!(
                            self.stdout,
                            SetBackgroundColor(Color::AnsiValue(236)),
                            SetForegroundColor(item_color)
                        )?;
                        let truncated = safe_truncate(&line.text, max_text_cols);
                        queue!(self.stdout, Print(truncated), ResetColor)?;
                        used_cols += truncated.chars().count();
                        let pad = (col_width as usize).saturating_sub(used_cols);
                        if pad > 0 {
                            queue!(self.stdout, ResetColor, Print(" ".repeat(pad)))?;
                        }
                    } else {
                        let truncated = safe_truncate(&line.text, max_text_cols);
                        queue!(self.stdout, SetForegroundColor(item_color), Print(truncated))?;
                        used_cols += truncated.chars().count();
                        let pad = (col_width as usize).saturating_sub(used_cols);
                        if pad > 0 {
                            queue!(self.stdout, ResetColor, Print(" ".repeat(pad)))?;
                        }
                    }
                }
            }

            // Draw vertical separator on the right (except for rightmost visible column)
            if v_idx + 1 < num_visible {
                let sep_x = col_x + col_width;
                for row_y in start_row..start_row + content_height as u16 {
                    queue!(
                        self.stdout,
                        MoveTo(sep_x, row_y),
                        SetForegroundColor(Color::DarkGrey),
                        Print("│"),
                        ResetColor
                    )?;
                }
            }
        }

        Ok(active_screen_x)
    }

    /// Draw status bar.
    fn draw_status_bar(
        &mut self,
        active_buffer: &DirectoryBuffer,
        active_col: usize,
        total_cols: usize,
        mode: &Mode,
        width: u16,
        row: u16,
    ) -> Result<()> {
        let (mode_bg, mode_fg) = match mode {
            Mode::Normal => (Color::Cyan, Color::Black),
            Mode::Insert => (Color::Green, Color::Black),
            Mode::VisualLine { .. } => (Color::Yellow, Color::Black),
            Mode::Command { .. } => (Color::Magenta, Color::Black),
            Mode::Search { .. } => (Color::Blue, Color::White),
            Mode::ConfirmModal { .. } => (Color::Red, Color::White),
            Mode::HelpModal { .. } => (Color::DarkGrey, Color::White),
        };

        queue!(
            self.stdout,
            MoveTo(0, row),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White)
        )?;

        let mode_tag = format!(" {} ", mode.name());
        queue!(
            self.stdout,
            SetBackgroundColor(mode_bg),
            SetForegroundColor(mode_fg),
            Print(&mode_tag),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White)
        )?;

        let item_info = format!(" {} items ", active_buffer.lines.len());
        let col_info = format!(" Column {}/{} ", active_col + 1, total_cols);
        let pos_info = format!(" Ln {}, Col {} ", active_buffer.cursor_row + 1, active_buffer.cursor_col + 1);

        let used = mode_tag.len() + item_info.len() + col_info.len() + pos_info.len();
        let spacer_len = (width as usize).saturating_sub(used);
        let spacer = " ".repeat(spacer_len);

        queue!(
            self.stdout,
            Print(item_info),
            Print(col_info),
            Print(spacer),
            Print(pos_info),
            ResetColor
        )?;

        Ok(())
    }

    /// Draw bottom command line bar.
    fn draw_command_line(
        &mut self,
        mode: &Mode,
        status_msg: Option<&str>,
        width: u16,
        row: u16,
    ) -> Result<()> {
        queue!(self.stdout, MoveTo(0, row))?;

        let mut printed_len = 0;
        match mode {
            Mode::Command { input } => {
                queue!(
                    self.stdout,
                    SetForegroundColor(Color::Yellow),
                    Print(":"),
                    SetForegroundColor(Color::White),
                    Print(input),
                    ResetColor
                )?;
                printed_len = 1 + input.chars().count();
            }
            Mode::Search { input } => {
                queue!(
                    self.stdout,
                    SetForegroundColor(Color::Yellow),
                    Print("/"),
                    SetForegroundColor(Color::White),
                    Print(input),
                    ResetColor
                )?;
                printed_len = 1 + input.chars().count();
            }
            _ => {
                if let Some(msg) = status_msg {
                    queue!(
                        self.stdout,
                        SetForegroundColor(Color::Yellow),
                        Print(msg),
                        ResetColor
                    )?;
                    printed_len = msg.chars().count();
                }
            }
        }

        let pad = (width as usize).saturating_sub(printed_len);
        if pad > 0 {
            queue!(self.stdout, Print(" ".repeat(pad)))?;
        }

        Ok(())
    }

    /// Draw interactive Diff Confirmation Popup across all modified directories.
    fn draw_confirm_modal(
        &mut self,
        plan: &FsPlan,
        scroll: usize,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let box_w = (width.saturating_sub(10)).min(90) as usize;
        let box_h = (height.saturating_sub(6)).min(24) as usize;

        let start_x = (width as usize).saturating_sub(box_w) / 2;
        let start_y = (height as usize).saturating_sub(box_h) / 2;

        let title = " Confirm Filesystem Changes Across Directories (:w) ";

        // Top Border
        queue!(
            self.stdout,
            MoveTo(start_x as u16, start_y as u16),
            SetBackgroundColor(Color::Black),
            SetForegroundColor(self.theme.border_color),
            Print("┌"),
            Print(format!("{:─^width$}", title, width = box_w - 2)),
            Print("┐")
        )?;

        let list_h = box_h.saturating_sub(4);
        let items: Vec<&crate::engine::FsAction> = plan.actions.iter().skip(scroll).take(list_h).collect();

        for (i, action) in items.iter().enumerate() {
            let row = start_y + 1 + i;
            let (badge_color, desc) = match action {
                crate::engine::FsAction::CreateFile { .. } => (self.theme.create_color, action.description()),
                crate::engine::FsAction::CreateDir { .. } => (self.theme.create_color, action.description()),
                crate::engine::FsAction::Rename { .. } => (self.theme.rename_color, action.description()),
                crate::engine::FsAction::Move { .. } => (self.theme.move_color, action.description()),
                crate::engine::FsAction::Delete { .. } => (self.theme.delete_color, action.description()),
            };

            let desc_chars = desc.chars().count();
            let max_w = box_w.saturating_sub(4);
            let truncated = if desc_chars > max_w {
                format!("{}...", safe_truncate(&desc, max_w.saturating_sub(3)))
            } else {
                safe_pad_truncate(&desc, max_w)
            };

            queue!(
                self.stdout,
                MoveTo(start_x as u16, row as u16),
                SetForegroundColor(self.theme.border_color),
                Print("│ "),
                SetForegroundColor(badge_color),
                Print(truncated),
                SetForegroundColor(self.theme.border_color),
                Print(" │")
            )?;
        }

        for i in items.len()..list_h {
            let row = start_y + 1 + i;
            queue!(
                self.stdout,
                MoveTo(start_x as u16, row as u16),
                SetForegroundColor(self.theme.border_color),
                Print("│"),
                Print(" ".repeat(box_w - 2)),
                Print("│")
            )?;
        }

        queue!(
            self.stdout,
            MoveTo(start_x as u16, (start_y + box_h - 2) as u16),
            SetForegroundColor(self.theme.border_color),
            Print("├"),
            Print("─".repeat(box_w - 2)),
            Print("┤")
        )?;

        let footer_text = " [y / Enter] Apply all changes     [n / Esc] Cancel ";
        queue!(
            self.stdout,
            MoveTo(start_x as u16, (start_y + box_h - 1) as u16),
            SetForegroundColor(self.theme.border_color),
            Print("│"),
            SetForegroundColor(Color::White),
            Print(format!("{:^width$}", footer_text, width = box_w - 2)),
            SetForegroundColor(self.theme.border_color),
            Print("│")
        )?;

        queue!(
            self.stdout,
            MoveTo(start_x as u16, (start_y + box_h) as u16),
            SetForegroundColor(self.theme.border_color),
            Print("└"),
            Print("─".repeat(box_w - 2)),
            Print("┘"),
            ResetColor
        )?;

        Ok(())
    }

    /// Draw help cheatsheet modal.
    fn draw_help_modal(&mut self, _scroll: usize, width: u16, height: u16) -> Result<()> {
        let box_w = 70.min(width as usize);
        let box_h = 16.min(height as usize);
        let start_x = (width as usize).saturating_sub(box_w) / 2;
        let start_y = (height as usize).saturating_sub(box_h) / 2;

        let title = " Euler Keybinding Cheatsheet ";
        let lines = [
            "  h               Move to parent directory column (left)",
            "  l               Enter directory / open file in $EDITOR",
            "  Ctrl-h / Ctrl-l Switch visible pane without changing layout",
            "  Enter           Enter directory / open file with xdg-open",
            "  j / k           Move down / up (updates child directory preview)",
            "  w / b / 0 / $   Word forward / backward / line start / line end",
            "  i / a           Insert mode before / after cursor",
            "  o / O           Insert line below / above (trailing / for dir)",
            "  dd              Delete line (marks for delete/trash on :w)",
            "  cw / C / cc     Change word / line end / whole line",
            "  u / Ctrl-r      Undo / Redo buffer edits or FS transactions",
            "  g.              Toggle hidden files (dotfiles)",
            "  gx              Open item with xdg-open (system opener)",
            "  :w              Review & commit all changes across open directories",
            "  :q / :q!        Quit / Force quit Euler",
        ];

        queue!(
            self.stdout,
            MoveTo(start_x as u16, start_y as u16),
            SetBackgroundColor(Color::Black),
            SetForegroundColor(self.theme.border_color),
            Print("┌"),
            Print(format!("{:─^width$}", title, width = box_w - 2)),
            Print("┐")
        )?;

        for (i, line) in lines.iter().enumerate() {
            if i >= box_h - 2 {
                break;
            }
            queue!(
                self.stdout,
                MoveTo(start_x as u16, (start_y + 1 + i) as u16),
                SetForegroundColor(self.theme.border_color),
                Print("│"),
                SetForegroundColor(Color::White),
                Print(format!("{:<width$}", line, width = box_w - 2)),
                SetForegroundColor(self.theme.border_color),
                Print("│")
            )?;
        }

        queue!(
            self.stdout,
            MoveTo(start_x as u16, (start_y + box_h - 1) as u16),
            SetForegroundColor(self.theme.border_color),
            Print("└"),
            Print(format!("{:─^width$}", " Press Esc or q to close ", width = box_w - 2)),
            Print("┘"),
            ResetColor
        )?;

        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Safely truncate string to at most `max_chars` Unicode characters (avoiding byte-splitting panics).
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Safely format string to exact visual width with space padding or safe truncation.
fn safe_pad_truncate(s: &str, width: usize) -> String {
    let char_count = s.chars().count();
    if char_count > width {
        safe_truncate(s, width).to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - char_count))
    }
}
