//! The `Branded<B>` nominal-typing primitive: Rust port of
//! `@deepseek-ai/dsh-brand`.
//!
//! A brand makes structurally-identical strings non-interchangeable at the
//! type level: a `SessionId` cannot be passed where a `CallId` is expected,
//! even though both are plain strings at runtime. Owning packages define a
//! marker enum plus an alias:
//!
//! ```ignore
//! #[doc(hidden)]
//! pub enum SessionIdTag {}
//! pub type SessionId = Branded<SessionIdTag>;
//! ```
//!
//! Comparison, logging, and serialization behave as ordinary strings
//! (serde-transparent, `Deref<Target = str>`).

use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;

/// A string carrying a compile-time-only brand `B`.
///
/// `Clone`/`Default` are implemented manually so the marker type `B` needs
/// no trait bounds (the marker is a phantom type parameter).
pub struct Branded<B>(String, PhantomData<fn() -> B>);

impl<B> Clone for Branded<B> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<B> Default for Branded<B> {
    fn default() -> Self {
        Self(String::new(), PhantomData)
    }
}

impl<B> PartialEq for Branded<B> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<B> Eq for Branded<B> {}

impl<B> PartialOrd for Branded<B> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<B> Ord for Branded<B> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<B> std::hash::Hash for Branded<B> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<B> Branded<B> {
    /// Plain cast construction (zero runtime cost).
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into(), PhantomData)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<B> From<String> for Branded<B> {
    fn from(value: String) -> Self {
        Self(value, PhantomData)
    }
}

impl<B> From<&str> for Branded<B> {
    fn from(value: &str) -> Self {
        Self(value.to_string(), PhantomData)
    }
}

impl<B> Deref for Branded<B> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<B> fmt::Display for Branded<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<B> fmt::Debug for Branded<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({:?})", std::any::type_name::<B>(), self.0)
    }
}

impl<B> serde::Serialize for Branded<B> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, B> serde::Deserialize<'de> for Branded<B> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(String::deserialize(deserializer)?, PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum SessionIdTag {}
    type SessionId = Branded<SessionIdTag>;

    #[test]
    fn brand_behaves_as_string() {
        let id = SessionId::new("abc");
        assert_eq!(id.as_str(), "abc");
        assert_eq!(id.to_string(), "abc");
        assert_eq!(id.len(), 3);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"abc\"");
        let back: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
