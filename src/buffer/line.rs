//! Buffer line and metadata representation.

use std::path::PathBuf;

/// Immutable identifier assigned to each filesystem item loaded from disk.
/// This disambiguates text renames from deletes + creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ItemMeta {
    pub file_type: FileType,
    pub size: u64,
    pub permissions: u32,
    pub modified: Option<std::time::SystemTime>,
    pub symlink_target: Option<PathBuf>,
}

/// A single editable line in the Euler directory buffer.
#[derive(Debug, Clone)]
pub struct BufferLine {
    /// `Some(id)` if this line represents an existing file loaded from disk.
    /// `None` if this line was newly created / typed by the user.
    pub entry_id: Option<EntryId>,

    /// The editable filename or relative path.
    pub text: String,

    /// Original filesystem metadata (if loaded from disk).
    pub meta: Option<ItemMeta>,

    /// Original absolute filesystem path if this entry originated from disk.
    pub source_path: Option<PathBuf>,
}

impl BufferLine {
    /// Create a line representing an existing file or directory on disk.
    pub fn from_disk(entry_id: EntryId, name: String, source_path: PathBuf, meta: ItemMeta) -> Self {
        Self {
            entry_id: Some(entry_id),
            text: name,
            meta: Some(meta),
            source_path: Some(source_path),
        }
    }

    /// Create a brand new uncommitted line entered by the user.
    pub fn new_empty() -> Self {
        Self {
            entry_id: None,
            text: String::new(),
            meta: None,
            source_path: None,
        }
    }

    /// Whether this entry represents a directory (either from disk or indicated by trailing '/').
    pub fn is_dir(&self) -> bool {
        if let Some(meta) = &self.meta {
            meta.file_type == FileType::Directory
        } else {
            self.text.ends_with('/')
        }
    }

    /// Whether this entry represents a symbolic link.
    pub fn is_symlink(&self) -> bool {
        self.meta
            .as_ref()
            .map(|m| m.file_type == FileType::Symlink)
            .unwrap_or(false)
    }

    /// Return the clean filename without trailing slash.
    pub fn clean_name(&self) -> &str {
        self.text.trim_end_matches('/')
    }
}
