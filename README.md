# Euler 📁
There are millions of terminal that uses vim keys but I feel like some parts are missing. So I am trying to create a file manager that acts like a neovim buffer . It takes inspirations from *mini.files*  and *oil.nvim* . I *use mini.fils* heavily in my work so I created this. This is mostly AI genarated now but I plan to read the code and fix any problem that might come or not I don't know. Let's see how this goes from now on.

# Why the name is Euler?
It's play on the word oil and tribute [Leonhard Euler](https://en.wikipedia.org/wiki/Leonhard_Euler) .

---
* The billow part is AI genarated .
---
A modern, buffer-centric terminal file manager written in Rust, inspired by the core philosophy of [oil.nvim](https://github.com/stevearc/oil.nvim): **edit your filesystem like a normal text buffer**.

Instead of interacting with files through rigid cursor selection and single-item shortcuts, Euler renders any directory as an editable text buffer. Renaming, creating, moving, and deleting files and directories are performed using standard Vim text editing motions, followed by a safe diff review (`:w`) before any changes touch the disk.

---

## Features

- **Multi-Column Side-by-Side Browsing (mini.files style):** Open and view up to 4 directories in a row horizontally (`[ Grandparent │ Parent │ Current │ Child ]`).
- **Free Navigation Without Forced Saves:** Move freely between directories and columns using `h` and `l`. In-flight buffer edits in any directory are preserved in memory without blocking or prompting you to commit before leaving.
- **Buffer-as-Filesystem:** Every open directory column is an editable text buffer. Edit file and directory names in place using standard Vim motions.
- **Smart Save Workflow (`:w`):** Creates, renames, and moves are applied immediately on `:w` without friction; the confirmation modal appears only if files or directories are being deleted (to trash).
- **Cycle & Collision Detection:** Safely handles cyclic renames (e.g. `A -> B` and `B -> A`) via staging swaps without data loss.
- **System Trash Integration:** Deletions default to the system trash following the XDG specification (reversible).
- **Filesystem Undo / Redo:** Use `u` in Normal mode to roll back committed filesystem batches.
- **Strict Vim Navigation:**
  - `h` navigates to the previous / parent directory (`..`).
  - `l` enters a directory or opens a file with `xdg-open`.
  - `<Enter>` enters a directory or opens a file in `$EDITOR`.
  - All arrow keys are explicitly disabled.
  - Hyphen (`-`) is inert.
  - `gx` opens the target with `xdg-open` (system default application).
- **Hidden Files by Default:** Dotfiles are visible by default and can be toggled on/off with `g.`.

---

## Installation & Build

Ensure you have Rust and Cargo installed:

```bash
git clone https://github.com/Mashrafidipto/euler.git
cd euler
cargo build --release
```

The compiled binary will be located at `target/release/euler`.

---

## Usage

```bash
# Open current directory
euler

# Open a specific directory
euler ~/Projects/my-app

# Display help and keybindings
euler --help
```

---

## Keybindings Quick Reference

### Normal Mode

| Key | Action |
| :--- | :--- |
| **`h`** | Move to parent / previous directory (`..`) *(with dirty buffer guard)* |
| **`l`** / **`<Enter>`** | Enter directory / open file in `$EDITOR` |
| **`j`** / **`k`** | Move cursor down / up |
| **`w`** / **`b`** | Jump forward / backward by word |
| **`0`** / **`$`** | Jump to start / end of filename |
| **`gg`** / **`G`** | Jump to first / last item |
| **`Ctrl-d`** / **`Ctrl-u`** | Scroll down / up half a page |
| **`i`** / **`a`** | Enter Insert mode before / after cursor |
| **`I`** / **`A`** | Jump to start / end of filename and enter Insert mode |
| **`o`** / **`O`** | Insert empty line below / above *(append `/` to create a directory)* |
| **`cw`** / **`ciw`** | Change word (erase word and enter Insert mode) |
| **`C`** | Change from cursor to line end |
| **`cc`** | Clear entire line and enter Insert mode |
| **`dd`** | Delete line *(marks file for deletion/trash on save)* |
| **`D`** | Clear line from cursor to end |
| **`x`** | Delete character under cursor |
| **`yy`** / **`p`** / **`P`** | Yank line / Paste line *(marks as duplicate/copy)* |
| **`V`** | Visual Line mode for multi-line deletion or yanking |
| **`u`** / **`Ctrl-r`** | Undo / Redo buffer edits or committed filesystem transactions |
| **`g.`** | Toggle hidden files (`.` dotfiles) |
| **`gx`** | Open selected item with `xdg-open` |
| **`:`** | Open command mode (`:w`, `:q`, `:e`, `:help`) |
| **`/`** | Search filenames (`n` / `N` for next / previous match) |

### Insert Mode

| Key | Action |
| :--- | :--- |
| **`Printable Chars`** | Direct text input into filename |
| **`Backspace`** | Delete character before cursor |
| **`Delete`** | Delete character under cursor |
| **`<Enter>`** | Insert new empty line below for creating an item |
| **`Ctrl-w`** | Delete previous word |
| **`Ctrl-u`** | Clear from cursor to line start |
| **`Esc`** | Exit Insert mode and return to Normal mode |

### Command Mode (`:`)

| Command | Action |
| :--- | :--- |
| **`:w`** | Compute diff, show confirmation modal, and commit changes |
| **`:w!`** | Force save directly without confirmation modal |
| **`:q`** | Quit Euler *(blocked if buffer has unsaved changes)* |
| **`:q!`** | Force quit Euler, discarding uncommitted edits |
| **`:wq`** / **`:x`** | Review, save, and exit |
| **`:e`** | Reload directory from disk, discarding uncommitted buffer edits |
| **`:help`** | Display keybinding cheatsheet |

---

## Configuration (`~/.config/euler/config.yaml`)

```yaml
# Whether hidden files (.dotfiles) are visible on launch
show_hidden: true

# Whether to show the diff confirmation modal on :w
confirm_diff: true

# Whether deleted files are moved to system trash (true) or permanently unlinked (false)
trash_deletes: true

# Natural sorting order for filenames
natural_sort: true

# Editor used when opening files
default_editor: "nvim"

# System desktop opener
opener: "xdg-open"

# Strictly disable all arrow keys across the application
disable_arrow_keys: true

# Highlight the full active line under the cursor (enabled by default)
cursorline: true
```

---

## License

MIT License. Copyright (c) 2026 Mashrafi Dipto.
