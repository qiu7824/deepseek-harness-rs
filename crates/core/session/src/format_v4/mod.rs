//! V4 wire conversion and row admission. Generation publication and lifecycle
//! relationships belong to the persistence/restore boundary, not these helpers.

mod admission;
mod catalog;
mod codec;
mod messages;
mod owned;
mod references;
mod relationships;
mod transform;
mod vocabulary;
mod wire;

pub use admission::{admit_v4_structural_fields, validate_v4_row_fields};
pub use catalog::historical_child_catalog_source;
pub use codec::{
    V4DecodeIssue, V4DecodeSummary, V4Decoder, V4Recovery, decode_v4_header, encode_v4_event,
    encode_v4_header,
};
pub use codec::{V4LogScan, V4LogScanner};
pub use messages::{convert_v3_event_messages, validate_v4_message_fields};
pub use owned::upgrade_v3_events;
pub use relationships::{V4ValidationSummary, V4Validator};
pub use transform::{V3Dialect, V3ToV4Transform, V4TransformSummary};
pub use vocabulary::V4Vocabulary;
pub use wire::decode_v3_header;

#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod codec_tests;
#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod relationship_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transform_tests;
