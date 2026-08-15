//! `LocalSpillStore`: the host-filesystem spill backend. Rust port of
//! `@deepseek-ai/dsh-spill-local`.

pub mod index;
pub mod invariant;
pub mod store;

pub use index::{Config, LocalSpillStore};
pub use store::{SaveTextOptions, SavedText, encode_segment, private_root, save_text_file, session_dir};
