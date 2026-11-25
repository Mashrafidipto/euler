//! Euler: A modal, buffer-centric TUI file manager inspired by oil.nvim.

mod app;
mod buffer;
mod config;
mod engine;
mod input;
mod ui;

use anyhow::Result;
use app::App;
use config::Config;
use std::path::PathBuf;
use ui::Terminal;

const HELP: &str = r#"Euler: A buffer-centric TUI file manager inspired by oil.nvim

Usage:
    euler [PATH]        Open directory (default: current directory)
    euler -h, --help    Show this help message
    euler -v, --version Show version

Keybindings (Normal Mode):
    h                   Navigate to parent directory (..)
    l, <Enter>          Enter directory / open file in $EDITOR
    j, k                Move cursor down / up (Arrow keys disabled)
    w, b                Jump word forward / backward
    0, $                Jump to start / end of filename
    i, a                Insert before / after cursor
    o, O                Insert line below / above (trailing / creates dir)
    dd                  Delete line (marks for deletion/trash)
    cw, C, cc           Change word / to end of line / whole line
    u, Ctrl-r           Undo / Redo buffer edits or FS transactions
    g.                  Toggle hidden files (dotfiles, visible by default)
    gx                  Open item with xdg-open
    :w                  Review diff and save changes to disk
    :q, :q!             Quit / Force quit
"#;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        match args[1].as_str() {
            "-h" | "--help" => {
                println!("{}", HELP);
                return Ok(());
            }
            "-v" | "--version" => {
                println!("Euler version {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {}
        }
    }

    let target_dir = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    let config = Config::load_or_default();
    let mut terminal = Terminal::init()?;
    let mut app = App::new(&target_dir, config)?;

    if let Err(e) = app.run(&mut terminal) {
        let _ = terminal.restore();
        eprintln!("Euler runtime error: {}", e);
    }

    terminal.restore()?;
    Ok(())
}
