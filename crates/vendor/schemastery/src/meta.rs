//! Validation options and schema metadata (port of `Schemastery.Options`
//! and `Schemastery.Meta`).

use std::sync::Arc;

use indexmap::IndexMap;

use crate::data::Data;

/// Predicate used to skip validation for selected values and schema nodes.
pub type Ignore = Arc<dyn Fn(&Data, &crate::schema::Schema) -> bool + Send + Sync>;

/// Object key or array index path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}

impl PathSeg {
    /// Format one segment (TS ValidationError prefix formatting).
    pub fn format(&self) -> String {
        match self {
            PathSeg::Key(key) => format!(".{key}"),
            PathSeg::Index(index) => format!("[{index}]"),
        }
    }
}

/// Runtime validation options shared by all schema calls.
#[derive(Clone, Default)]
pub struct Options {
    /// Remove invalid object properties instead of throwing when possible.
    pub autofix: bool,
    /// Skip validation for selected values and schema nodes.
    pub ignore: Option<Ignore>,
    /// Path used to format nested validation errors.
    pub path: Vec<PathSeg>,
}

impl Options {
    /// Clone with one appended path segment (TS `{ ...options, path: [...] }`).
    pub fn with_path(&self, segment: PathSeg) -> Options {
        let mut path = self.path.clone();
        path.push(segment);
        Options {
            autofix: self.autofix,
            ignore: self.ignore.clone(),
            path,
        }
    }
}

/// Deprecated/experimental badge attached to a schema node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub r#type: String,
}

/// Regexp constraint recorded on string schemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub source: String,
    pub flags: Option<String>,
}

/// Localized or plain schema description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Desc {
    Plain(String),
    Localized(IndexMap<String, String>),
}

impl Desc {
    pub fn is_empty(&self) -> bool {
        match self {
            Desc::Plain(text) => text.is_empty(),
            Desc::Localized(map) => map.is_empty(),
        }
    }
}

/// UI and validation metadata attached by schema builder methods.
#[derive(Clone, Default, Debug)]
pub struct Meta {
    pub default: Option<Data>,
    pub required: bool,
    pub disabled: bool,
    pub collapse: bool,
    pub hidden: bool,
    pub loose: bool,
    pub badges: Vec<Badge>,
    pub role: Option<String>,
    pub extra: Option<Data>,
    pub link: Option<String>,
    pub description: Option<Desc>,
    pub comment: Option<String>,
    pub pattern: Option<Pattern>,
    pub max: Option<f64>,
    pub min: Option<f64>,
    pub step: Option<f64>,
}
