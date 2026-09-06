//! The embedded reconciler (T-0191, CC-03, CC-54, CC-55, MF-11).
//!
//! The Portal carries `jcctl` as a library rather than shelling out to it: the same loader
//! that validates a repository on the command line validates it here, so the Portal cannot
//! disagree with the CLI about what a manifest is.
//!
//! One replica reconciles. [`leader`] elects it with a PostgreSQL advisory lock, and
//! [`daemon`] is the loop that reads the repository and refreshes the live-state mirror.

pub mod daemon;
pub mod leader;

pub use daemon::{SyncError, SyncStatus, Syncer};
pub use leader::Leadership;
