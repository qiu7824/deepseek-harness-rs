//! Browse backend of the directory-picker seam: one-level directory listing
//! and child-directory creation over the host filesystem. Rust port of
//! `packages/host/directory-picker-browse`.

pub mod index;
pub mod invariant;

pub use index::{
    BrowseDirectoryPicker, BrowseDirectoryPickerPlugin, Config, ListingCandidate, NAME,
    ancestry_crumbs, bounded_insert, create_directory, fully_qualified, home_dir, platform,
    race_abort, windows_drive_entries,
};
