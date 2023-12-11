use infinitree::{fields, ChunkPointer, Digest};
pub mod tree;
pub use tree::*;
mod files;
pub use files::*;
mod zfs_snapshots;
pub use zfs_snapshots::*;
pub mod rollsum;
pub mod splitter;
mod stash;

pub use stash::list_snapshots::ZfsSnapshotList;
pub use stash::restore;
pub use stash::store;

type ChunkIndex = fields::VersionedMap<Digest, ChunkPointer>;
type FileIndex = fields::VersionedMap<String, Entry>;
type ZfsIndex = fields::VersionedMap<String, ZfsSnapshot>;

#[derive(Clone, Default, infinitree::Index)]
pub struct Files {
    pub chunks: ChunkIndex,
    pub files: FileIndex,
    pub zfs_snapshots: ZfsIndex,
    pub tree: Tree,
}
