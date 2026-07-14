//! Manifest format for `.gim` files.
//!
//! The `.gim` file is the entry point for unpacking. It contains all
//! metadata needed to reconstruct the game: snapshots, file entries,
//! CAS object offsets, compression settings.

use serde::{Deserialize, Serialize};

/// Top-level manifest structure. Serialized as JSON to `.gim` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimManifest {
    /// Manifest format version (currently 1).
    pub version: u32,
    /// Game metadata.
    pub game: GimGameInfo,
    /// Per-game config (hash algorithm, etc.).
    pub config: serde_json::Value,
    /// Compression settings used during repack.
    pub compression: GimCompressionInfo,
    /// Snapshot entries (one per snapshot in the chain).
    pub snapshots: Vec<GimSnapshot>,
    /// CAS object entries (deduplicated, stored in objects.bin).
    pub objects: GimObjectsFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimGameInfo {
    pub title: String,
    pub alias: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimCompressionInfo {
    /// Profile name: "fast", "balanced", "max".
    pub profile: String,
    /// Compression level (1-10).
    pub level: u32,
    /// xtool codecs used (e.g. ["zlib", "preflate"]).
    pub codecs: Vec<String>,
    /// Chunk size used by xtool (e.g. "64mb").
    pub chunk_size: String,
    /// xtool version string.
    pub xtool_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimSnapshot {
    pub id: String,
    pub parent: Option<String>,
    pub timestamp: i64,
    pub message: Option<String>,
    pub file_count: i64,
    pub added_size: i64,
    /// Data file name (e.g. "original.bin").
    pub data_file: String,
    /// Files in this snapshot.
    pub files: Vec<GimFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimFile {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimObjectsFile {
    /// File name (e.g. "objects.bin").
    pub file: String,
    /// Object entries with offsets into the file.
    pub entries: Vec<GimObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GimObject {
    pub hash: String,
    /// Offset into objects.bin (precompressed data).
    pub offset: u64,
    /// Size of precompressed data.
    pub compressed_size: u64,
    /// Original (uncompressed) size.
    pub orig_size: u64,
}

impl GimManifest {
    /// Serialize to JSON pretty-printed.
    pub fn to_json(&self) -> GResult<String> {
        serde_json::to_string_pretty(self).map_err(|e| {
            crate::error::GError::Other(format!("manifest serialize: {e}"))
        })
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> GResult<Self> {
        serde_json::from_str(json).map_err(|e| {
            crate::error::GError::Other(format!("manifest parse: {e}"))
        })
    }

    /// Find a snapshot by ID.
    pub fn find_snapshot(&self, id: &str) -> Option<&GimSnapshot> {
        self.snapshots.iter().find(|s| s.id == id)
    }
}

use crate::error::GResult;
