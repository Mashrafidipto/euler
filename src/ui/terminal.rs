//! Terminal lifecycle and buffered rendering engine.

use super::theme::Theme;
use crate::buffer::DirectoryBuffer;
use crate::engine::FsPlan;
use crate::input::Mode;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::io::{stdout, BufWriter, Stdout, Write};

pub struct Terminal {
    stdout: BufWriter<Stdout>,
    pub theme: Theme,
}

impl Terminal {
    /// Initialize the terminal into raw mode, alternate screen, and buffered I/O.
    pub fn init() -> Result<Self> {
        enable_raw_mode()?;
        let mut raw_stdout = stdout();
        execute!(raw_stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
        raw_stdout.flush()?;

        // 64 KB buffer allows an entire 4K terminal frame to be buffered and flushed in 1 syscall
        let stdout = BufWriter::with_capacity(64 * 1024, raw_stdout);

        Ok(Self {
            stdout,
            theme: Theme::default(),
        })
    }

    /// Restore terminal to normal settings on exit.
    pub fn restore(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(self.stdout, Show, LeaveAlternateScreen)?;
        self.stdout.flush()?;
        Ok(())
    }

    /// Get current terminal dimensions (width, height).
    pub fn size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    /// Render the entire Euler application view with double-buffered atomic flush.
    pub fn render(
        &mut self,
        buffer: &mut DirectoryBuffer,
        mode: &Mode,
        pending_plan: Option<&FsPlan>,
        status_msg: Option<&str>,
    ) -> Result<()> {
        let (width, height) = self.size();
        if width < 20 || height < 6 {
            return Ok(()); // Window too small
        }

        // Viewport bounds
        let header_row = 0;
        let content_start_row = 1;
        let content_height = height.saturating_sub(3) as usize; // reserve header, status, command line
        let status_row = height.saturating_sub(2);
        let cmd_row = height.saturating_sub(1);

        // Adjust scroll offset to keep cursor in view
        if buffer.cursor_row < buffer.scroll_offset {
            buffer.scroll_offset = buffer.cursor_row;
        } else if buffer.cursor_row >= buffer.scroll_offset + content_height {
            buffer.scroll_offset = buffer.cursor_row + 1 - content_height;
        }

        // 1. Draw Header
        self.draw_header(buffer, width, header_row)?;

        // 2. Draw Directory Buffer Lines
        self.draw_buffer_lines(buffer, mode, width, content_start_row, content_height)?;

        // 3. Draw Status Bar
        self.draw_status_bar(buffer, mode, width, status_row)?;

        // 4. Draw Command Line / Message Bar
        self.draw_command_line(mode, status_msg, width, cmd_row)?;

        // 5. Draw Overlays / Modals (if active)
        match mode {
            Mode::ConfirmModal { scroll } => {
                if let Some(plan) = pending_plan {
                    self.draw_confirm_modal(plan, *scroll, width, height)?;
                }
            }
            Mode::DirtyNavPrompt { target, .. } => {
                self.draw_dirty_prompt(target, width, height)?;
            }
            Mode::HelpModal { scroll } => {
                self.draw_help_modal(*scroll, width, height)?;
            }
            _ => {}
        }

        // Position hardware cursor for Insert / Command / Search modes
        match mode {
            Mode::Insert => {
                let line_num_width = 4;
                let prefix_width = 3; // "📁 " / "📄 "
                let cursor_x = (line_num_width + prefix_width + buffer.cursor_col) as u16;
                let cursor_y = content_start_row + (buffer.cursor_row.saturating_sub(buffer.scroll_offset)) as u16;
                queue!(self.stdout, Show, MoveTo(cursor_x, cursor_y))?;
            }
            Mode::Command { input } => {
                queue!(
                    self.stdout,
                    Show,
                    MoveTo(1 + input.chars().count() as u16, cmd_row)
                )?;
            }
            Mode::Search { input } => {
                queue!(
                    self.stdout,
                    Show,
                    MoveTo(1 + input.chars().count() as u16, cmd_row)
                )?;
            }
            _ => {
                queue!(self.stdout, Hide)?;
            }
        }

        // Single atomic flush for the whole frame (eliminates tearing and flicker!)
        self.stdout.flush()?;
        Ok(())
    }

    /// Draw top header bar.
    fn draw_header(&mut self, buffer: &DirectoryBuffer, width: u16, row: u16) -> Result<()> {
        queue!(
            self.stdout,
            MoveTo(0, row),
            SetBackgroundColor(Color::DarkBlue),
            SetForegroundColor(Color::White)
        )?;

        let dirty_mark = if buffer.is_dirty() { " [+]" } else { "" };
        let header_text = format!(" 📁 {}{}", buffer.current_dir.display(), dirty_mark);
        let padded = format!("{:<width$}", header_text, width = width as usize);

        queue!(self.stdout, Print(&padded[..width as usize]), ResetColor)?;
        Ok(())
    }

    /// Draw buffer text lines with space padding (overwrites old frame without flickering line clears).
    fn draw_buffer_lines(
        &mut self,
        buffer: &DirectoryBuffer,
        mode: &Mode,
        width: u16,
        start_row: u16,
        content_height: usize,
    ) -> Result<()> {
        let visual_range = match mode {
            Mode::VisualLine { start_row: start } => {
                let (low, high) = if *start <= buffer.cursor_row {
                    (*start, buffer.cursor_row)
                } else {
                    (buffer.cursor_row, *start)
                };
                Some(low..=high)
            }
            _ => None,
        };

        for i in 0..content_height {
            let line_idx = buffer.scroll_offset + i;
            let screen_row = start_row + i as u16;

            queue!(self.stdout, MoveTo(0, screen_row))?;

            if line_idx >= buffer.lines.len() {
                // Empty line below buffer, pad remainder of line
                let empty_pad = " ".repeat((width as usize).saturating_sub(1));
                queue!(
                    self.stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print("~"),
                    Print(empty_pad),
                    ResetColor
                )?;
                continue;
            }

            let line = &buffer.lines[line_idx];
            let is_cursor_row = line_idx == buffer.cursor_row;
            let is_visual_selected = visual_range.as_ref().map(|r| r.contains(&line_idx)).unwrap_or(false);

            // 1. Line Number (width = 4)
            let line_num_str = format!("{:>3} ", line_idx + 1);
            queue!(
                self.stdout,
                SetForegroundColor(self.theme.line_number_color),
                Print(&line_num_str),
                ResetColor
            )?;

            // 2. Icon / Type prefix (width = 3)
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

            queue!(self.stdout, SetForegroundColor(item_color), Print(icon))?;

            let mut used_cols = 4 + 3; // line number + icon

            // 3. Line Text & Cursor Rendering
            if is_visual_selected {
                queue!(
                    self.stdout,
                    SetBackgroundColor(Color::DarkMagenta),
                    SetForegroundColor(Color::White),
                    Print(&line.text),
                    ResetColor
                )?;
                used_cols += line.text.chars().count();
            } else if is_cursor_row && *mode == Mode::Normal {
                // Render Normal Mode block cursor on the character
                let chars: Vec<char> = line.text.chars().collect();
                if chars.is_empty() {
                    queue!(
                        self.stdout,
                        SetBackgroundColor(Color::White),
                        SetForegroundColor(Color::Black),
                        Print(" "),
                        ResetColor
                    )?;
                    used_cols += 1;
                } else {
                    for (c_idx, &ch) in chars.iter().enumerate() {
                        if c_idx == buffer.cursor_col {
                            queue!(
                                self.stdout,
                                SetBackgroundColor(Color::White),
                                SetForegroundColor(Color::Black),
                                Print(ch),
                                ResetColor
                            )?;
                        } else {
                            queue!(self.stdout, SetForegroundColor(item_color), Print(ch))?;
                        }
                    }
                    used_cols += chars.len();
                }
            } else {
                // Regular rendering
                queue!(
                    self.stdout,
                    SetForegroundColor(item_color),
                    Print(&line.text),
                    ResetColor
                )?;
                used_cols += line.text.chars().count();
            }

            // Pad remainder of the screen line with spaces to overwrite previous content without Clear()
            let padding_len = (width as usize).saturating_sub(used_cols);
            if padding_len > 0 {
                queue!(self.stdout, ResetColor, Print(" ".repeat(padding_len)))?;
            }
        }

        Ok(())
    }

    /// Draw status bar.
    fn draw_status_bar(
        &mut self,
        buffer: &DirectoryBuffer,
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
            Mode::DirtyNavPrompt { .. } => (Color::Red, Color::White),
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

        let item_info = format!(" {} items ", buffer.lines.len());
        let pos_info = format!(" Ln {}, Col {} ", buffer.cursor_row + 1, buffer.cursor_col + 1);

        let used = mode_tag.len() + item_info.len() + pos_info.len();
        let spacer_len = (width as usize).saturating_sub(used);
        let spacer = " ".repeat(spacer_len);

        queue!(
            self.stdout,
            Print(item_info),
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

        // Overwrite the rest of the command line with spaces
        let pad = (width as usize).saturating_sub(printed_len);
        if pad > 0 {
            queue!(self.stdout, Print(" ".repeat(pad)))?;
        }

        Ok(())
    }

    /// Draw the interactive Diff Confirmation Popup.
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

        let title = " Confirm Filesystem Changes (:w) ";

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

            let truncated = if desc.len() > box_w - 4 {
                format!("{}...", &desc[..box_w - 7])
            } else {
                format!("{:<width$}", desc, width = box_w - 4)
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

        // Fill empty rows if plan has fewer items than modal height
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

        // Divider
        queue!(
            self.stdout,
            MoveTo(start_x as u16, (start_y + box_h - 2) as u16),
            SetForegroundColor(self.theme.border_color),
            Print("├"),
            Print("─".repeat(box_w - 2)),
            Print("┤")
        )?;

        // Footer Actions
        let footer_text = " [y / Enter] Apply changes     [n / Esc] Cancel ";
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

        // Bottom Border
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

    /// Draw dirty navigation guard prompt.
    fn draw_dirty_prompt(&mut self, target: &std::path::Path, width: u16, height: u16) -> Result<()> {
        let box_w = 60.min(width as usize);
        let box_h = 6;
        let start_x = (width as usize).saturating_sub(box_w) / 2;
        let start_y = (height as usize).saturating_sub(box_h) / 2;

        let target_str = target.file_name().unwrap_or_default().to_string_lossy();
        let prompt = format!("Unsaved changes! Navigate to '{}'?", target_str);
        let options = "[w] Save & Go    [d] Discard & Go    [Esc] Stay";

        queue!(
            self.stdout,
            MoveTo(start_x as u16, start_y as u16),
            SetBackgroundColor(Color::DarkRed),
            SetForegroundColor(Color::White),
            Print("┌──────────────────────────────────────────────────────────┐"),
            MoveTo(start_x as u16, (start_y + 1) as u16),
            Print(format!("│{:^58}│", prompt)),
            MoveTo(start_x as u16, (start_y + 2) as u16),
            Print("│                                                          │"),
            MoveTo(start_x as u16, (start_y + 3) as u16),
            SetForegroundColor(Color::Yellow),
            Print(format!("│{:^58}│", options)),
            SetForegroundColor(Color::White),
            MoveTo(start_x as u16, (start_y + 4) as u16),
            Print("└──────────────────────────────────────────────────────────┘"),
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
            "  h               Move to parent directory (..) ",
            "  l / Enter       Enter directory / open file in $EDITOR",
            "  j / k           Move down / up (Arrow keys disabled)",
            "  w / b / 0 / $   Word forward / backward / line start / line end",
            "  i / a           Insert mode before / after cursor",
            "  o / O           Insert line below / above (trailing / for dir)",
            "  dd              Delete line (marks for delete/trash)",
            "  cw / C / cc     Change word / line end / whole line",
            "  u / Ctrl-r      Undo / Redo buffer edits or FS transactions",
            "  g.              Toggle hidden files (dotfiles)",
            "  gx              Open item with xdg-open (system opener)",
            "  :w              Review changes diff and save to disk",
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
