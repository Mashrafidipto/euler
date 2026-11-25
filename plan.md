# Euler: Architecture & Implementation Plan for an Oil.nvim-like TUI File Manager

Euler is a modern, standalone terminal file manager built from scratch in Rust, inspired by the core philosophy of [oil.nvim](https://github.com/stevearc/oil.nvim): **edit your filesystem like a normal text buffer**.

Instead of interacting with files through rigid cursor menus and single-item shortcuts, Euler renders a directory as an editable buffer of text lines. Renaming, creating, moving, and deleting files and folders are performed using standard Vim text editing motions, followed by a safe diff review triggered by `:w` before changes touch the disk.

---

## 1. System Architecture

Euler adopts a clean, decoupled **State-Action-View** architecture powered by `ratatui` and `crossterm`:

```
┌────────────────────────────────────────────────────────────────────────┐
│                              Terminal UI                               │
│              (Ratatui Frame: Buffer View, Status Bar, Modal)           │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Keyboard Events (Arrow keys disabled)
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                           Input Router                                 │
│        Mode Dispatcher: Normal ── Insert ── Visual ── Command ── Diff  │
│        (Strict Vim motions: h/j/k/l; hyphen & arrow keys disabled)     │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Buffer Mutations
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                        Directory Buffer Engine                         │
│   Baseline Snapshot (Original FS) <---> Live Buffer Lines              │
│   (Persistent EntryId tracking to detect Renames vs Creates/Deletes)   │
│   (Hidden files visible by default)                                    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Save Triggered via `:w`
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                      Reconciliation & Diff Engine                      │
│   - Map live lines to baseline entries                                 │
│   - Generate FsPlan: Creates, Renames, Moves, Deletes                  │
│   - Pre-flight checks: cycle detection, collision detection            │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ User Confirmed (y / <CR>)
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                       Filesystem Action Executor                       │
│   - Topological execution (create dirs -> stage swaps -> rename -> del)│
│   - Safe Trash integration (`trash` crate)                             │
│   - Transaction history for Undo / Redo (`u` / `Ctrl-r`)               │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Concepts & Subsystems

### 2.1. Persistent Line Tracking (`EntryId`)
In a pure text buffer, when a user edits text:
* Changing `old.txt` to `new.txt` $\rightarrow$ **Rename**.
* Deleting a line $\rightarrow$ **Delete**.
* Adding a new line $\rightarrow$ **Create**.

To disambiguate edits, every entry loaded from disk is assigned an immutable internal `EntryId`. When the user types a new line (`o`, `O`), its `entry_id` is `None`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryId(pub usize);

#[derive(Debug, Clone)]
pub struct BufferLine {
    /// None if newly typed; Some(id) if loaded from disk
    pub entry_id: Option<EntryId>,
    /// The editable filename or relative path
    pub text: String,
    /// Metadata cached from original file
    pub is_dir: bool,
    pub size: u64,
    pub permissions: u32,
    pub modified: Option<std::time::SystemTime>,
}
```

### 2.2. Modal Editing System & Navigation Rules
Euler operates with strict Vim modes:
1. **Normal Mode**:
   - **Directory Traversal:**
     - `h` $\rightarrow$ Navigate to previous / parent directory (`..`). Prompt if buffer has uncommitted edits.
     - `l` (and `<CR>`) $\rightarrow$ If cursor is on a directory, enter it. If on a file, open in `$EDITOR`.
     - `-` (hyphen) $\rightarrow$ Explicitly disabled (does nothing).
     - Arrow keys (`↑`, `↓`, `←`, `→`) $\rightarrow$ Explicitly disabled across all modes.
   - **Line Motions:** `j` (down), `k` (up), `w`, `b`, `0`, `$`, `gg`, `G`, `Ctrl-d`, `Ctrl-u`.
   - **Entering Insert Mode:** `i`, `a`, `I`, `A`, `o` (insert line below), `O` (insert line above).
   - **Line Operations:** `dd` (mark for delete/trash), `yy` (yank), `p` (paste/duplicate), `cw`, `C`, `cc`, `r`.
   - **External Opener:** `gx` $\rightarrow$ Open the selected file, folder, or target using `xdg-open` (system default handler) in a detached process.
   - **Undo / Redo:** `u` (undo buffer edit or filesystem transaction), `Ctrl-r` (redo).
   - **Toggling Hidden Files:** `g.` (hidden files are **visible by default**).
2. **Insert Mode**:
   - Direct typing, character deletion (`Backspace`, `Delete`), line splitting (`<CR>`), and `Esc` to return to Normal mode.
   - Arrow keys disabled.
3. **Visual Line Mode (`V`)**:
   - Multi-line block selection for deletion (`d`) and yanking (`y`).
4. **Command Mode (`:`)**:
   - `:w` $\rightarrow$ Compute diff, display Diff Review Modal, and commit upon confirmation.
   - `:w!` $\rightarrow$ Force save without confirmation modal.
   - `:q` $\rightarrow$ Quit (blocks if dirty).
   - `:q!` $\rightarrow$ Force quit (discards edits).
   - `:e` $\rightarrow$ Reload directory from disk.
5. **Diff Confirmation Modal**:
   - Triggered exclusively by `:w`.
   - Displays color-coded preview of pending actions (`+ CREATE`, `~ RENAME`, `→ MOVE`, `- DELETE`).
   - `y` / `<CR>` confirms and executes; `n` / `Esc` aborts and returns to buffer.
   - `j` / `k` scrolls the changes list.

### 2.3. Reconciliation & Diff Engine
When `:w` is invoked:
1. **Deletions**: Any `EntryId` in the baseline snapshot not present in the active buffer is marked for deletion.
2. **Creations**: Any line with `entry_id: None` is marked for creation (ends with `/` $\rightarrow$ directory, otherwise file).
3. **Renames / Moves**: Any line with an `EntryId` whose text differs from the baseline is marked for rename. If path contains `/`, it is treated as a move.
4. **Safety & Cycle Resolution**:
   - **Cycles**: If `a.txt -> b.txt` and `b.txt -> a.txt`, stage through a temporary path to prevent data destruction.
   - **Collisions**: Check if any destination already exists on disk and is not being freed by another action.
   - **Hierarchy**: Auto-create parent directories in order (`mkdir -p`) before creating nested files.

### 2.4. Filesystem Executor & Undo Stack
* Deletions default to system trash via the cross-platform `trash` crate (reversible).
* Actions are applied in dependency-sorted order.
* Successfully committed batches are pushed onto an application-level **Undo Stack**, allowing `u` in Normal mode to roll back filesystem operations.

---

## 3. Project Directory Structure

```
euler/
├── Cargo.toml
├── plan.md
├── planed_keywords.md
└── src/
    ├── main.rs               # CLI entrypoint, argument parsing, terminal init
    ├── app.rs                # Application loop, top-level state, mode switching
    ├── buffer/
    │   ├── mod.rs
    │   ├── line.rs           # BufferLine, EntryId, line formatting
    │   └── dir_buffer.rs     # DirectoryBuffer (lines, baseline, cursor, scroll)
    ├── engine/
    │   ├── mod.rs
    │   ├── diff.rs           # Diff calculation (Baseline vs Live lines)
    │   ├── planner.rs        # Cycle resolution, collision checks, topological sort
    │   ├── executor.rs       # FS execution (create, rename, move, trash)
    │   └── undo.rs           # Multi-action filesystem undo/redo stack
    ├── input/
    │   ├── mod.rs
    │   ├── mode.rs           # Mode enum (Normal, Insert, VisualLine, Command, Confirm)
    │   ├── handler.rs        # Key event router (disables arrows, handles h/l/j/k)
    │   └── motions.rs        # Vim motion logic (word jumps, line jumps, find char)
    ├── ui/
    │   ├── mod.rs
    │   ├── layout.rs         # Screen layouts (main buffer, status bar, command line)
    │   ├── buffer_view.rs    # Buffer renderer (line numbers, cursor, file colors)
    │   ├── status_bar.rs     # Current mode, active path, modified flag, item count
    │   └── confirm_popup.rs  # Interactive Diff Confirmation dialog
    └── config/
        ├── mod.rs
        └── settings.rs       # User configuration (show_hidden=true, disable_arrows=true)
```

---

## 4. Implementation Phases

### Phase 1: Foundation & Terminal Setup
* Set up `Cargo.toml` with `ratatui`, `crossterm`, `walkdir`, `trash`.
* Implement terminal raw mode lifecycle and main event loop in `main.rs` & `app.rs`.
* Configure event router: explicitly drop/ignore all Arrow key events across all modes.
* Build basic screen layout (buffer window, status bar, command bar).

### Phase 2: Directory Buffer & Baseline Snapshot
* Implement reading directory contents into `DirectoryBuffer`.
* Set `show_hidden: true` by default (dotfiles displayed on launch, toggleable with `g.`).
* Assign unique `EntryId`s to disk entries and preserve a baseline snapshot.
* Render directory lines with file icons and type styling (directory, symlink, file).

### Phase 3: Text Editing & Vim Motions
* Implement cursor navigation in Normal mode (`h` moves up/prev, `l` enters dir/opens file, `j` down, `k` up, `w`, `b`, `0`, `$`, `gg`, `G`).
* Ensure `-` (hyphen) is unmapped / inert.
* Implement Insert mode (`i`, `a`, `A`, `I`, `o`, `O`) with inline text editing and backspace.
* Implement line deletions (`dd`, `D`) and Visual Line mode (`V`).
* Add buffer-level dirty state flag (`[+]`) when live text differs from baseline.

### Phase 4: Reconciliation Diff & Planner
* Build `diff.rs` to compare `baseline` against `lines`.
* Classify actions into `CreateFile`, `CreateDir`, `Delete`, `Rename`, `Move`.
* Implement cycle detection (swapping through temp file) and collision guards.
* Unit test edge cases (cyclic renames, nested folder creates, multi-line deletes).

### Phase 5: Diff Review Modal & FS Execution
* Render the interactive Diff Confirmation Popup when `:w` is executed.
* Show clear color-coded actions (`+ CREATE`, `~ RENAME`, `→ MOVE`, `- DELETE`).
* Implement `executor.rs` to commit approved actions to disk.
* Connect deletions to `trash` crate.
* Re-read directory into a fresh baseline upon successful commit.

### Phase 6: Navigation & Guardrails
* Pressing `l` or `<CR>` on a directory opens it as a new buffer.
* Pressing `h` navigates to previous / parent directory (`..`).
* Pressing `l` or `<CR>` on a file opens it in `$EDITOR`.
* **Dirty Buffer Guard**: Prompt user before navigating away if buffer has uncommitted changes.

### Phase 7: Polish & History
* Add filesystem-level undo (`u`) and redo (`Ctrl-r`).
* Implement `:q`, `:q!`, `:e` in command mode.
* Configuration file (`config.toml`) with `show_hidden = true` and `disable_arrow_keys = true`.
