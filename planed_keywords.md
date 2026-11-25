# Planned Keywords & Keybindings for Euler

This document specifies all the **keywords, keybindings, commands, and configuration options** planned for Euler based on your specifications.

---

## 1. Normal Mode Keybindings

Normal Mode is the primary navigation and command mode.

| Key / Keyword | Category | Planned Action | Notes / Oil.nvim Equivalent |
| :--- | :--- | :--- | :--- |
| **`h`** | Directory Navigation | **Move to parent / previous directory column (left)** | Freely navigates left; all in-flight edits preserved in memory |
| **`l`** | Directory Navigation | **Enter directory column (right) / Open file** in `$EDITOR` | Opens/focuses adjacent column or opens file |
| **`j`** | Navigation | Move cursor down one line | Standard Vim motion |
| **`k`** | Navigation | Move cursor up one line | Standard Vim motion |
| **`w`** | Navigation | Jump forward to the start of the next word | In filename |
| **`b`** | Navigation | Jump backward to the start of the previous word | In filename |
| **`0`** | Navigation | Jump to the beginning of the filename | Standard Vim motion |
| **`$`** | Navigation | Jump to the end of the filename | Standard Vim motion |
| **`gg`** | Navigation | Jump to the first entry in the directory | Standard Vim motion |
| **`G`** | Navigation | Jump to the last entry in the directory | Standard Vim motion |
| **`Ctrl-d`** | Navigation | Scroll down half a page | Standard Vim scroll |
| **`Ctrl-u`** | Navigation | Scroll up half a page | Standard Vim scroll |
| **`<CR>`** (Enter) | Directory Navigation | Enter directory / Open file in `$EDITOR` | Alias to `l` |
| **`-`** (Hyphen) | **Disabled** | **Does nothing** | Disabled by specification |
| **`↑` / `↓` / `←` / `→`** | **Disabled** | **All arrow keys are completely disabled** | Strict Vim keys (`h`/`j`/`k`/`l`) enforced |
| **`i`** | Insert Mode | Enter Insert mode before cursor to edit filename | Standard Vim insert |
| **`a`** | Insert Mode | Enter Insert mode after cursor to edit filename | Standard Vim append |
| **`I`** | Insert Mode | Jump to start of filename and enter Insert mode | Standard Vim insert |
| **`A`** | Insert Mode | Jump to end of filename and enter Insert mode | Standard Vim append |
| **`o`** | Insert Mode | Insert a new empty line **below** current line to create a file/folder | Trailing `/` creates directory |
| **`O`** | Insert Mode | Insert a new empty line **above** current line to create a file/folder | Trailing `/` creates directory |
| **`cw`** / **`ciw`** | Text Editing | Change word (erase word and enter Insert mode) | Standard Vim change |
| **`C`** | Text Editing | Change from cursor to the end of the line | Standard Vim change |
| **`cc`** | Text Editing | Erase entire filename and enter Insert mode | Standard Vim change line |
| **`r`** | Text Editing | Replace single character under cursor | Standard Vim replace |
| **`dd`** | Deletion | Delete current line *(marks file for deletion/trash on `:w`)* | Standard Vim delete line |
| **`D`** | Deletion | Clear text from cursor to end of line | Standard Vim delete |
| **`x`** | Deletion | Delete character under cursor | Standard Vim delete char |
| **`yy`** | Clipboard | Yank (copy) current line to internal register | Standard Vim yank |
| **`p`** | Clipboard | Paste yanked line below current line *(marks as duplicate/copy)* | Standard Vim paste |
| **`P`** | Clipboard | Paste yanked line above current line | Standard Vim paste |
| **`V`** | Visual Mode | Enter Visual Line mode to select multiple items | Standard Vim visual line |
| **`u`** | Undo | Undo last buffer edit OR last committed filesystem transaction | Standard Vim undo |
| **`Ctrl-r`** | Redo | Redo last undone buffer edit or transaction | Standard Vim redo |
| **`:`** | Command Mode | Open bottom command bar for `:` commands (e.g. `:w`, `:q`) | Standard Vim command mode |
| **`/`** | Search | Open search prompt to jump to matching filenames | Standard Vim search |
| **`n`** | Search | Jump to next search match | Standard Vim search next |
| **`N`** | Search | Jump to previous search match | Standard Vim search prev |
| **`g.`** | Toggle | Toggle visibility of hidden files (`.` dotfiles) | **Visible by default** |
| **`gx`** | External Opener | **Open file, folder, or target with `xdg-open`** (system default desktop app) | Standard Vim `gx` / system opener |

> [!NOTE]
> `<C-s>` and `<C-c><C-c>` are **disabled**. Saving and reviewing diffs is performed exclusively via **`:w`**.

---

## 2. Insert Mode Keybindings

Insert Mode allows typing and editing filenames directly in the buffer.

| Key / Keyword | Category | Planned Action | Notes |
| :--- | :--- | :--- | :--- |
| **`Printable Characters`** | Text Input | Inserts the typed character into filename at cursor position | Standard text input |
| **`Backspace`** | Text Input | Deletes character before cursor | Standard backspace |
| **`Delete`** | Text Input | Deletes character under cursor | Standard delete |
| **`<CR>`** (Enter) | Line Split | Inserts a new empty line below for creating an item | Trailing `/` creates directory |
| **`Esc`** / **`Ctrl-[`** | Mode Switch | Exit Insert mode and return to Normal mode | Standard Vim mode switch |
| **`Ctrl-w`** | Editing | Delete previous word | Standard Vim line edit |
| **`Ctrl-u`** | Editing | Clear from cursor to start of line | Standard Vim line edit |
| **`↑` / `↓` / `←` / `→`** | **Disabled** | **All arrow keys are completely disabled** | Arrow keys do nothing in Insert mode |

---

## 3. Visual Line Mode Keybindings

Visual Line Mode (`V`) is used to operate on blocks of files at once.

| Key / Keyword | Category | Planned Action |
| :--- | :--- | :--- |
| **`j`** / **`k`** | Selection | Extend selection down / up |
| **`d`** / **`x`** | Deletion | Delete all selected lines *(marks all selected files for deletion/trash)* |
| **`y`** | Clipboard | Yank all selected lines into internal register |
| **`Esc`** / **`Ctrl-[`** | Mode Switch | Cancel selection and return to Normal mode |
| **`↑` / `↓` / `←` / `→`** | **Disabled** | **All arrow keys are completely disabled** |

---

## 4. Diff Confirmation Modal Keybindings

When `:w` is invoked, this modal displays a color-coded review of all proposed actions:
* `+ CREATE FILE` / `+ CREATE DIR` (Green)
* `~ RENAME` (Yellow)
* `→ MOVE` (Cyan)
* `- DELETE` (Red, to trash)

| Key / Keyword | Category | Planned Action |
| :--- | :--- | :--- |
| **`y`** / **`<CR>`** | Confirmation | Apply all changes atomically to disk and reload buffer |
| **`n`** / **`Esc`** / **`q`** | Cancellation | Abort changes; return to the buffer to keep editing |
| **`j`** / **`k`** | Scrolling | Scroll the list of pending changes if it exceeds screen height |
| **`↑` / `↓` / `←` / `→`** | **Disabled** | **All arrow keys are completely disabled** |

---

## 5. Command-Line (`:`) Keywords

Typing `:` in Normal mode opens the bottom command bar.

| Command / Keyword | Arguments | Planned Action |
| :--- | :--- | :--- |
| **`:w`** / **`:write`** | None | **Saves changes immediately; prompts for confirmation only if items are being deleted** |
| **`:w!`** | None | Force save: commits changes directly without confirmation modal regardless of deletions |
| **`:q`** / **`:quit`** | None | Quit Euler. If buffer is dirty (unsaved edits), blocks and warns |
| **`:q!`** | None | Force quit Euler immediately, discarding all unsaved buffer changes |
| **`:wq`** / **`:x`** | None | Review & save changes via diff modal; exit Euler once applied |
| **`:e`** / **`:edit`** | None / `[path]` | Reload active directory from disk (discards uncommitted buffer changes), or navigate to `[path]` |
| **`:cd`** | `[path]` | Change working directory to `[path]` (default: `$HOME`) |
| **`:trash`** | None | Open system trash directory in a buffer |
| **`:help`** / **`:h`** | None | Display help manual with keybinding overview |
| **`:set cursorline`** / **`:set cul`** | None | Enable full cursor line highlighting |
| **`:set nocursorline`** / **`:set nocul`** | None | Disable full cursor line highlighting |

---

## 6. Configuration Keywords (`config.toml`)

These configuration keywords allow personalizing Euler's behavior.

| Config Keyword | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| **`show_hidden`** | `boolean` | **`true`** | **Hidden files (`.dotfiles`) are visible by default.** Toggle with `g.` |
| **`confirm_diff`** | `boolean` | `true` | If `true`, `:w` displays the confirmation modal whenever files/directories are being deleted |
| **`trash_deletes`** | `boolean` | `true` | If `true`, deleted files are moved to system trash. If `false`, permanently unlinks |
| **`natural_sort`** | `boolean` | `true` | If `true`, sorts filenames using natural version order (e.g. `2.txt` before `10.txt`) |
| **`auto_save_on_nav`**| `boolean` | `false` | If `true`, automatically prompts to save when navigating into/out of a directory |
| **`default_editor`** | `string` | `"$EDITOR"` | Command used to open files when pressing `l` or `<CR>` on a file |
| **`opener`** | `string` | `"xdg-open"` | System application used by `gx` to open any file or folder |
| **`columns`** | `list` | `["icon", "permissions", "size", "mtime"]` | Columns displayed in the directory buffer alongside filenames |
| **`line_numbers`** | `string` | `"relative"` | Options: `"relative"`, `"absolute"`, `"none"` |
| **`theme`** | `string` | `"default"` | Color theme for file types, diff modal, and status bar |
| **`cursorline`** | `boolean` | **`true`** | **Highlight the entire line containing the cursor (Vim `cursorline`). Enabled by default.** |
| **`disable_arrow_keys`**| `boolean`| **`true`** | **Explicitly disable all arrow keys across the entire app** |
