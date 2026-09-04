//! Domain data form: schema-validated, change-emitting KV domains over
//! storage backends. Rust port of `@deepseek-ai/dsh-storage-domain`.
//!
//! Consumers declare a [`DomainSpec`] once and open it through the
//! [`DomainFacility`]; reads are synchronous from memory, every write
//! queues on the domain's single write chain (backend durability first,
//! then memory, then the `domain/changed` event).

pub mod domain;
pub mod error;
pub mod events;
pub mod facility;
pub mod spec;

pub use domain::{Domain, DomainGlobal, KvTable};
pub use error::{DomainError, DomainErrorCode};
pub use events::DomainChanged;
pub use facility::{DomainFacility, DomainFacilityConfig};
pub use spec::{
    DomainGlobalSpec, DomainSpec, DomainTableSpec, InvalidRecordPolicy, RecordSchema,
    define_domain, define_domain_with_options, descriptor_of, domain_global, domain_table,
};

// The backend contract's single home is the storage hub.
pub use dsh_storage::{
    KvFacet, KvLayout, KvUnit, KvUnitDescriptor, KvUnitSnapshot, StorageBackend, unit_name_matches,
};
