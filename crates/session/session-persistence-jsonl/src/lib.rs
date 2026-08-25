//! JSONL durable session-persistence backend: one append-only file per
//! session, concatenated checksummed Zstandard frames (or UTF-8 plaintext).
//! Rust port of `@deepseek-ai/dsh-session-persistence-jsonl`.

pub mod format;
pub mod index;
mod packed_stream;
pub mod zstd;

pub use format::{
    HeaderLine, JsonlCompression, ScannerCheckpoint, SessionLogScan, SessionLogScanner,
    encode_segment, event_lines, from_header_line, log_path, log_suffix, parse_header_meta,
    project_dir, project_key, scan_log, session_dir, to_header_line,
};
pub use index::{JsonlConfig, JsonlSessionPersistence, JsonlTornMarker, parse_config};
pub use zstd::{
    ZstdFrameRange, ZstdFrameScan, compress_zstd_frame, decode_zstd_frames,
    decode_zstd_header_line, decompress_zstd_frame, decompress_zstd_prefix, scan_zstd_frames,
};
