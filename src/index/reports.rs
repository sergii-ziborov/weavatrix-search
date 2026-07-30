use super::{ContentVisitReport, MultiContentVisitReport, PathBuf};

/// Stable metadata returned after a full index build.
#[derive(Debug)]
pub struct IndexBuildReport {
    /// Indexed roots in insertion order.
    pub roots: Vec<PathBuf>,
    /// Selected files retained by the index.
    pub files: u64,
    /// Raw source bytes retained by the index.
    pub content_bytes: u64,
    /// Deterministic root/path/content revision.
    pub revision: String,
    /// Scanner evidence for the full build.
    pub scan: MultiContentVisitReport,
}

/// Stable metadata returned after applying one watcher plan.
#[derive(Debug)]
pub struct IndexUpdateReport {
    /// Newly selected paths.
    pub added: u64,
    /// Existing paths whose bytes were replaced.
    pub updated: u64,
    /// Existing paths removed or no longer selected.
    pub removed: u64,
    /// Selected files retained after the update.
    pub files: u64,
    /// Raw source bytes retained after the update.
    pub content_bytes: u64,
    /// New deterministic index revision.
    pub revision: String,
    /// Whether conservative watcher evidence required a complete rebuild.
    pub full_rebuild: bool,
    /// Changed-file scanner evidence when no rebuild was required.
    pub changed_scan: Option<ContentVisitReport>,
}

/// Cheap status suitable for health endpoints and live-index diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    /// Indexed roots in insertion order.
    pub roots: Vec<PathBuf>,
    /// Selected files retained by the index.
    pub files: u64,
    /// Raw source bytes retained by the index.
    pub content_bytes: u64,
    /// Deterministic root/path/content revision.
    pub revision: String,
}
