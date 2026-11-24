//! Minimal Rust core for HamokiSync. This crate never accesses the OS clipboard
//! and never connects to production unless explicitly used by a caller.
//! Production-facing GUI and native clipboard integration are separate layers.

pub mod history;
pub mod policy;
pub mod protocol;

pub use history::{History, HistoryItem, ItemData};
pub use policy::{Decision, Identity, SyncPolicy};
pub use protocol::{ClientError, ProtocolClient, Snapshot, SnapshotKind};
