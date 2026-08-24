//! Schema node types, builders, and the validation engine (port of the
//! main `Schema` implementation).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use parking_lot::Mutex;
use regex::Regex;

use crate::data::{Data, PathKey, js_number_string};
use crate::error::{Issue, StandardResult, ValidationError};
use crate::meta::{Badge, Desc, Meta, Options, PathSeg, Pattern};

static NEXT_UID: AtomicU64 = AtomicU64::new(0);

fn next_uid() -> u64 {
    NEXT_UID.fetch_add(1, Ordering::Relaxed)
}

/// Callback used by `transform` nodes: converts a validated value
/// (TS `Schema.transform(inner, callback, preserve)`).
pub type TransformFn = Arc<dyn Fn(&Data, &Options) -> Result<Data, ValidationError> + Send + Sync>;

/// Deferred recursive schema builder (TS `Schema.lazy(builder)`).
pub type LazyFn = Arc<dyn Fn() -> Schema + Send + Sync>;

/// Schema node (the TS `type` field plus its payload fields).
pub enum Node {
    Any,
    Never,
    Const(Data),
    String,
    Number,
    Boolean,
    Bitset(IndexMap<String, u64>),
    Function,
    Is(String),
    Array(Box<Schema>),
    Dict {
        inner: Box<Schema>,
        s_key: Box<Schema>,
    },
    Tuple(Vec<Schema>),
    Object(IndexMap<String, Schema>),
    Union(Vec<Schema>),
    Intersect(Vec<Schema>),
    Transform {
        inner: Box<Schema>,
        callback: TransformFn,
        preserve: bool,
    },
    Lazy {
        builder: LazyFn,
        cache: Mutex<Option<Box<Schema>>>,
    },
}

impl Clone for Node {
    fn clone(&self) -> Self {
        match self {
            Node::Any => Node::Any,
            Node::Never => Node::Never,
            Node::Const(value) => Node::Const(value.clone()),
            Node::String => Node::String,
            Node::Number => Node::Number,
            Node::Boolean => Node::Boolean,
            Node::Bitset(bits) => Node::Bitset(bits.clone()),
            Node::Function => Node::Function,
            Node::Is(name) => Node::Is(name.clone()),
            Node::Array(inner) => Node::Array(inner.clone()),
            Node::Dict { inner, s_key } => Node::Dict {
                inner: inner.clone(),
                s_key: s_key.clone(),
            },
            Node::Tuple(list) => Node::Tuple(list.clone()),
            Node::Object(dict) => Node::Object(dict.clone()),
            Node::Union(list) => Node::Union(list.clone()),
            Node::Intersect(list) => Node::Intersect(list.clone()),
            Node::Transform {
                inner,
                callback,
                preserve,
            } => Node::Transform {
                inner: inner.clone(),
                callback: callback.clone(),
                preserve: *preserve,
            },
            // A derived schema starts with an empty lazy cache; TS shallow
            // copies share the built inner, but rebuilding on demand is
            // equivalent and simpler to share safely.
            Node::Lazy { builder, .. } => Node::Lazy {
                builder: builder.clone(),
                cache: Mutex::new(None),
            },
        }
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Any => write!(f, "Any"),
            Node::Never => write!(f, "Never"),
            Node::Const(value) => write!(f, "Const({value:?})"),
            Node::String => write!(f, "String"),
            Node::Number => write!(f, "Number"),
            Node::Boolean => write!(f, "Boolean"),
            Node::Bitset(bits) => write!(f, "Bitset({bits:?})"),
            Node::Function => write!(f, "Function"),
            Node::Is(name) => write!(f, "Is({name})"),
            Node::Array(inner) => write!(f, "Array({inner:?})"),
            Node::Dict { inner, s_key } => write!(f, "Dict({inner:?}, {s_key:?})"),
            Node::Tuple(list) => write!(f, "Tuple({list:?})"),
            Node::Object(dict) => write!(f, "Object({dict:?})"),
            Node::Union(list) => write!(f, "Union({list:?})"),
            Node::Intersect(list) => write!(f, "Intersect({list:?})"),
            Node::Transform {
                inner, preserve, ..
            } => {
                write!(f, "Transform({inner:?}, preserve={preserve})")
            }
            Node::Lazy { .. } => write!(f, "Lazy(..)"),
        }
    }
}

/// Immutable schema node shared by all derived builders (TS schema objects
/// are shallow-cloned per builder call; Rust shares the payload `Arc`).
pub(crate) struct SchemaInner {
    pub uid: u64,
    pub meta: Meta,
    pub node: Node,
}

/// A validation schema. Builders return new schemas sharing the node while
/// overlaying metadata, mirroring TS `Schema(this)` shallow copies.
#[derive(Clone)]
pub struct Schema {
    pub(crate) inner: Arc<SchemaInner>,
}

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Schema#{} {} {:?}",
            self.inner.uid,
            self.type_name(),
            self.inner.meta
        )
    }
}

impl Schema {
    fn from_node(node: Node) -> Schema {
        Schema {
            inner: Arc::new(SchemaInner {
                uid: next_uid(),
                meta: Meta::default(),
                node,
            }),
        }
    }

    /// Shallow copy with a fresh uid (TS `Schema(this)`).
    fn derive(&self) -> Schema {
        Schema {
            inner: Arc::new(SchemaInner {
                uid: next_uid(),
                meta: self.inner.meta.clone(),
                node: self.inner.node.clone(),
            }),
        }
    }

    /// Unique schema id (TS `uid`).
    pub fn uid(&self) -> u64 {
        self.inner.uid
    }

    /// Schema metadata.
    pub fn meta(&self) -> &Meta {
        &self.inner.meta
    }

    /// Node payload.
    pub fn node(&self) -> &Node {
        &self.inner.node
    }

    /// Serialize this schema as the reference-preserving wire envelope used by
    /// the browser form renderer (`{ uid, refs }`). Shared nodes are emitted
    /// once and every edge is represented by its uid.
    pub fn to_json(&self) -> serde_json::Value {
        fn meta_json(meta: &Meta) -> serde_json::Value {
            let mut out = serde_json::Map::new();
            if let Some(value) = meta.default.as_ref().and_then(Data::to_json) {
                out.insert("default".to_string(), value);
            }
            for (key, value) in [
                ("required", meta.required),
                ("disabled", meta.disabled),
                ("collapse", meta.collapse),
                ("hidden", meta.hidden),
                ("loose", meta.loose),
            ] {
                if value {
                    out.insert(key.to_string(), serde_json::Value::Bool(true));
                }
            }
            if !meta.badges.is_empty() {
                out.insert(
                    "badges".to_string(),
                    serde_json::Value::Array(
                        meta.badges
                            .iter()
                            .map(|badge| serde_json::json!({"text": badge.text, "type": badge.r#type}))
                            .collect(),
                    ),
                );
            }
            if let Some(role) = &meta.role {
                out.insert("role".to_string(), role.clone().into());
            }
            if let Some(extra) = meta.extra.as_ref().and_then(Data::to_json) {
                out.insert("extra".to_string(), extra);
            }
            if let Some(link) = &meta.link {
                out.insert("link".to_string(), link.clone().into());
            }
            if let Some(description) = &meta.description {
                let value = match description {
                    Desc::Plain(text) => serde_json::Value::String(text.clone()),
                    Desc::Localized(entries) => serde_json::Value::Object(
                        entries
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone().into()))
                            .collect(),
                    ),
                };
                out.insert("description".to_string(), value);
            }
            if let Some(comment) = &meta.comment {
                out.insert("comment".to_string(), comment.clone().into());
            }
            if let Some(pattern) = &meta.pattern {
                out.insert(
                    "pattern".to_string(),
                    serde_json::json!({
                        "source": pattern.source,
                        "flags": pattern.flags,
                    }),
                );
            }
            for (key, value) in [("max", meta.max), ("min", meta.min), ("step", meta.step)] {
                if let Some(value) = value.and_then(serde_json::Number::from_f64) {
                    out.insert(key.to_string(), serde_json::Value::Number(value));
                }
            }
            serde_json::Value::Object(out)
        }

        fn visit(schema: &Schema, refs: &mut serde_json::Map<String, serde_json::Value>) {
            let key = schema.uid().to_string();
            if refs.contains_key(&key) {
                return;
            }
            // Reserve first so shared/recursive graphs terminate.
            refs.insert(key.clone(), serde_json::Value::Null);
            let mut node = serde_json::Map::new();
            node.insert("type".to_string(), schema.type_name().into());
            let meta = meta_json(schema.meta());
            if meta.as_object().is_some_and(|meta| !meta.is_empty()) {
                node.insert("meta".to_string(), meta);
            }
            let uid = |child: &Schema| serde_json::Value::Number(child.uid().into());
            match schema.node() {
                Node::Any
                | Node::Never
                | Node::String
                | Node::Number
                | Node::Boolean
                | Node::Function => {}
                Node::Const(value) => {
                    node.insert(
                        "value".to_string(),
                        value.to_json().unwrap_or(serde_json::Value::Null),
                    );
                }
                Node::Bitset(bits) => {
                    node.insert(
                        "bits".to_string(),
                        serde_json::to_value(bits).unwrap_or_default(),
                    );
                }
                Node::Is(name) => {
                    node.insert("constructor".to_string(), name.clone().into());
                }
                Node::Array(inner) => {
                    visit(inner, refs);
                    node.insert("inner".to_string(), uid(inner));
                }
                Node::Dict { inner, s_key } => {
                    visit(inner, refs);
                    visit(s_key, refs);
                    node.insert("inner".to_string(), uid(inner));
                    node.insert("sKey".to_string(), uid(s_key));
                }
                Node::Tuple(list) | Node::Union(list) | Node::Intersect(list) => {
                    for child in list {
                        visit(child, refs);
                    }
                    node.insert(
                        "list".to_string(),
                        serde_json::Value::Array(list.iter().map(uid).collect()),
                    );
                }
                Node::Object(dict) => {
                    for child in dict.values() {
                        visit(child, refs);
                    }
                    node.insert(
                        "dict".to_string(),
                        serde_json::Value::Object(
                            dict.iter()
                                .map(|(name, child)| (name.clone(), uid(child)))
                                .collect(),
                        ),
                    );
                }
                Node::Transform {
                    inner, preserve, ..
                } => {
                    visit(inner, refs);
                    node.insert("inner".to_string(), uid(inner));
                    if *preserve {
                        node.insert("preserve".to_string(), true.into());
                    }
                }
                Node::Lazy { builder, cache } => {
                    let mut cache = cache.lock();
                    let inner = cache.get_or_insert_with(|| Box::new(builder()));
                    visit(inner, refs);
                    node.insert("inner".to_string(), uid(inner));
                }
            }
            refs.insert(key, serde_json::Value::Object(node));
        }

        let mut refs = serde_json::Map::new();
        visit(self, &mut refs);
        serde_json::json!({"uid": self.uid(), "refs": refs})
    }

    /// TS `schema.type`.
    pub fn type_name(&self) -> &'static str {
        match &self.inner.node {
            Node::Any => "any",
            Node::Never => "never",
            Node::Const(_) => "const",
            Node::String => "string",
            Node::Number => "number",
            Node::Boolean => "boolean",
            Node::Bitset(_) => "bitset",
            Node::Function => "function",
            Node::Is(_) => "is",
            Node::Array(_) => "array",
            Node::Dict { .. } => "dict",
            Node::Tuple(_) => "tuple",
            Node::Object(_) => "object",
            Node::Union(_) => "union",
            Node::Intersect(_) => "intersect",
            Node::Transform { .. } => "transform",
            Node::Lazy { .. } => "lazy",
        }
    }

    // ---- node builders (TS `defineMethod` factories) ----

    pub fn any() -> Schema {
        Self::from_node(Node::Any)
    }

    pub fn never() -> Schema {
        Self::from_node(Node::Never)
    }

    /// TS `Schema.const` (Rust keyword workaround).
    pub fn constant(value: Data) -> Schema {
        Self::from_node(Node::Const(value))
    }

    pub fn string() -> Schema {
        Self::from_node(Node::String)
    }

    pub fn number() -> Schema {
        Self::from_node(Node::Number)
    }

    /// Non-negative integer numbers.
    pub fn natural() -> Schema {
        Self::number().step(1.0).min(0.0)
    }

    /// A number between 0 and 1 marked as a slider.
    pub fn percent() -> Schema {
        Self::number()
            .step(0.01)
            .min(0.0)
            .max(1.0)
            .role("slider", None)
    }

    pub fn boolean() -> Schema {
        Self::from_node(Node::Boolean)
    }

    pub fn bitset(bits: IndexMap<String, u64>) -> Schema {
        Self::from_node(Node::Bitset(bits)).default(Data::Number(0.0))
    }

    pub fn function() -> Schema {
        Self::from_node(Node::Function)
    }

    /// Accept instances whose constructor name matches (TS
    /// `Schema.is(Constructor)`; Rust reduces class identity to names).
    pub fn is(name: &str) -> Schema {
        Self::from_node(Node::Is(name.to_string()))
    }

    pub fn array(inner: Schema) -> Schema {
        Self::from_node(Node::Array(Box::new(inner))).default(Data::Array(Vec::new()))
    }

    pub fn dict(inner: Schema, s_key: Option<Schema>) -> Schema {
        Self::from_node(Node::Dict {
            inner: Box::new(inner),
            s_key: Box::new(s_key.unwrap_or_else(Self::string)),
        })
        .default(Data::Object(IndexMap::new()))
    }

    pub fn tuple(list: Vec<Schema>) -> Schema {
        Self::from_node(Node::Tuple(list)).default(Data::Array(Vec::new()))
    }

    pub fn object(dict: IndexMap<String, Schema>) -> Schema {
        Self::from_node(Node::Object(dict)).default(Data::Object(IndexMap::new()))
    }

    pub fn union(list: Vec<Schema>) -> Schema {
        Self::from_node(Node::Union(list))
    }

    pub fn intersect(list: Vec<Schema>) -> Schema {
        Self::from_node(Node::Intersect(list))
    }

    pub fn transform(inner: Schema, callback: TransformFn, preserve: bool) -> Schema {
        Self::from_node(Node::Transform {
            inner: Box::new(inner),
            callback,
            preserve,
        })
    }

    pub fn lazy(builder: LazyFn) -> Schema {
        Self::from_node(Node::Lazy {
            builder,
            cache: Mutex::new(None),
        })
    }

    /// Accept `Date` instances or parse datetime strings into `Date` objects.
    pub fn date() -> Schema {
        Self::union(vec![
            Self::is("Date"),
            Self::transform(
                Self::string().role("datetime", None),
                Arc::new(|value: &Data, options: &Options| match value {
                    Data::String(text) => DateTime::parse_from_rfc3339(text)
                        .map(|date| Data::Date(date.with_timezone(&Utc)))
                        .map_err(|_| {
                            ValidationError::new(format!("invalid date \"{text}\""), options)
                        }),
                    _ => unreachable!("string schema"),
                }),
                true,
            ),
        ])
    }

    /// Accept `RegExp` instances or parse strings into regular expressions.
    pub fn reg_exp(flag: &str) -> Schema {
        let flag = flag.to_string();
        Self::union(vec![
            Self::is("RegExp"),
            Self::transform(
                Self::string().role("regexp", Some(Data::String(flag.clone()))),
                Arc::new(move |value: &Data, options: &Options| match value {
                    Data::String(source) => {
                        if build_regex(source, &flag).is_none() {
                            return Err(ValidationError::new(
                                format!("invalid regexp \"{source}\""),
                                options,
                            ));
                        }
                        Ok(Data::RegExp {
                            source: source.clone(),
                            flags: flag.clone(),
                        })
                    }
                    _ => unreachable!("string schema"),
                }),
                true,
            ),
        ])
    }

    /// Accept binary sources and normalize them to byte buffers.
    pub fn array_buffer(encoding: Option<&str>) -> Schema {
        let mut members = vec![
            Self::is("ArrayBuffer"),
            Self::is("SharedArrayBuffer"),
            Self::transform(
                Self::any(),
                Arc::new(|value: &Data, _options: &Options| match value {
                    Data::Binary(_) => Ok(value.clone()),
                    _ => Err(ValidationError::new(
                        format!("expected ArrayBufferSource but got {value}"),
                        _options,
                    )),
                }),
                true,
            ),
        ];
        if let Some(encoding) = encoding {
            let encoding = encoding.to_string();
            members.push(Self::transform(
                Self::string(),
                Arc::new(move |value: &Data, _options: &Options| match value {
                    Data::String(text) => {
                        // decoding is lenient (mirrors Buffer.from), so no
                        // validation error path exists here
                        let bytes = if encoding == "base64" {
                            dsh_cosmokit::from_base64(text)
                        } else {
                            dsh_cosmokit::from_hex(text)
                        };
                        Ok(Data::Binary(bytes))
                    }
                    _ => unreachable!("string schema"),
                }),
                true,
            ));
        }
        Self::union(members)
    }

    /// Infer a schema from a primitive value (TS `Schema.from`; class and
    /// function sources have no Rust equivalent and are not accepted).
    pub fn from(source: Option<&Data>) -> Schema {
        match source {
            None => Self::any(),
            Some(data @ (Data::String(_) | Data::Number(_) | Data::Bool(_))) => {
                Self::constant(data.clone()).required(true)
            }
            Some(other) => panic!("cannot infer schema from {other}"),
        }
    }
    // ---- metadata chain methods (TS builder methods) ----

    fn with_meta(&self, update: impl FnOnce(&mut Meta)) -> Schema {
        let mut schema = self.derive();
        let inner = Arc::get_mut(&mut schema.inner).expect("derived schema is unique");
        update(&mut inner.meta);
        schema
    }

    pub fn required(&self, value: bool) -> Schema {
        self.with_meta(|meta| meta.required = value)
    }

    pub fn hidden(&self, value: bool) -> Schema {
        self.with_meta(|meta| meta.hidden = value)
    }

    pub fn loose(&self, value: bool) -> Schema {
        self.with_meta(|meta| meta.loose = value)
    }

    pub fn disabled(&self, value: bool) -> Schema {
        self.with_meta(|meta| meta.disabled = value)
    }

    pub fn collapse(&self, value: bool) -> Schema {
        self.with_meta(|meta| meta.collapse = value)
    }

    pub fn role(&self, text: &str, extra: Option<Data>) -> Schema {
        self.with_meta(|meta| {
            meta.role = Some(text.to_string());
            meta.extra = extra;
        })
    }

    pub fn link(&self, link: &str) -> Schema {
        self.with_meta(|meta| meta.link = Some(link.to_string()))
    }

    pub fn default(&self, value: Data) -> Schema {
        self.with_meta(|meta| meta.default = Some(value))
    }

    pub fn comment(&self, text: &str) -> Schema {
        self.with_meta(|meta| meta.comment = Some(text.to_string()))
    }

    pub fn description(&self, text: &str) -> Schema {
        self.with_meta(|meta| meta.description = Some(Desc::Plain(text.to_string())))
    }

    pub fn max(&self, value: f64) -> Schema {
        self.with_meta(|meta| meta.max = Some(value))
    }

    pub fn min(&self, value: f64) -> Schema {
        self.with_meta(|meta| meta.min = Some(value))
    }

    pub fn step(&self, value: f64) -> Schema {
        self.with_meta(|meta| meta.step = Some(value))
    }

    pub fn pattern(&self, source: &str, flags: Option<&str>) -> Schema {
        let source = source.to_string();
        let flags = flags.map(|f| f.to_string());
        self.with_meta(move |meta| meta.pattern = Some(Pattern { source, flags }))
    }

    pub fn deprecated(&self) -> Schema {
        self.with_meta(|meta| {
            meta.badges.push(Badge {
                text: "deprecated".to_string(),
                r#type: "danger".to_string(),
            });
        })
    }

    pub fn experimental(&self) -> Schema {
        self.with_meta(|meta| {
            meta.badges.push(Badge {
                text: "experimental".to_string(),
                r#type: "warning".to_string(),
            });
        })
    }

    /// Attach arbitrary metadata (TS `extra(key, value)` sets any meta key;
    /// Rust stores it in the single `extra` slot).
    pub fn extra(&self, key: &str, value: Data) -> Schema {
        let mut map = IndexMap::new();
        map.insert(key.to_string(), value);
        self.with_meta(move |meta| meta.extra = Some(Data::Object(map)))
    }

    /// Add or replace an object property schema (TS `set(key, value)`).
    pub fn set(&self, key: &str, value: Schema) -> Schema {
        let mut schema = self.derive();
        let inner = Arc::get_mut(&mut schema.inner).expect("derived schema is unique");
        match &mut inner.node {
            Node::Object(dict) => {
                dict.insert(key.to_string(), value);
            }
            _ => panic!("Schema.set() is only valid on object schemas"),
        }
        schema
    }

    /// Append a tuple, union, or intersection member schema (TS `push`).
    pub fn push(&self, value: Schema) -> Schema {
        let mut schema = self.derive();
        let inner = Arc::get_mut(&mut schema.inner).expect("derived schema is unique");
        match &mut inner.node {
            Node::Tuple(list) | Node::Union(list) | Node::Intersect(list) => {
                list.push(value);
            }
            _ => panic!("Schema.push() is only valid on tuple/union/intersect schemas"),
        }
        schema
    }

    // ---- resolution (TS `Schema.resolve`) ----

    /// Validate and normalize `data` against `schema`, mutating the input in
    /// place where the TS runtime writes adaptations back.
    ///
    /// Returns the normalized output value (TS returns `[value, adapted?]`;
    /// the adapted write-back happens inside nested property resolution).
    pub fn resolve(
        data: &mut Data,
        schema: &Schema,
        options: &Options,
        strict: bool,
    ) -> Result<Data, ValidationError> {
        if let Some(ignore) = &options.ignore
            && ignore(data, schema)
        {
            return Ok(data.clone());
        }
        if data.is_nullish() && !matches!(schema.inner.node, Node::Lazy { .. }) {
            if schema.inner.meta.required {
                return Err(ValidationError::new("missing required value", options));
            }
            let mut current: Option<&Schema> = Some(schema);
            let mut fallback = schema.inner.meta.default.clone();
            while let Some(schema) = current {
                match &schema.inner.node {
                    Node::Intersect(list) if fallback.as_ref().is_none_or(|f| f.is_nullish()) => {
                        current = list.first();
                        fallback = current.and_then(|c| c.inner.meta.default.clone());
                    }
                    _ => break,
                }
            }
            match fallback {
                Some(value) if !value.is_nullish() => *data = value,
                _ => return Ok(data.clone()),
            }
        }
        match resolve_node(data, schema, options, strict) {
            Ok(value) => Ok(value),
            Err(error) => {
                if schema.inner.meta.loose {
                    Ok(schema.inner.meta.default.clone().unwrap_or(Data::Undefined))
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Validate without input mutation write-backs (`Schema(data)`).
    pub fn validate(schema: &Schema, mut data: Data) -> Result<Data, ValidationError> {
        Schema::resolve(&mut data, schema, &Options::default(), false)
    }

    // ---- simplify / i18n / formatting ----

    /// Remove values equal to schema defaults from normalized output
    /// (TS `simplify`).
    pub fn simplify(&self, value: &Data) -> Data {
        // TS: `if (deepEqual(value, this.meta.default, this.type === 'dict'))
        // return null` — the third argument is the *strict* flag.
        if Data::deep_equal(
            value,
            self.meta().default.as_ref().unwrap_or(&Data::Undefined),
            self.type_name() == "dict",
        ) {
            return Data::Null;
        }
        if value.is_nullish() {
            return value.clone();
        }
        match &self.inner.node {
            Node::Object(_) | Node::Dict { .. } => {
                let Data::Object(map) = value else {
                    return value.clone();
                };
                let mut result = IndexMap::new();
                for (key, item) in map {
                    let schema = match &self.inner.node {
                        Node::Object(dict) => dict.get(key).cloned(),
                        Node::Dict { inner, .. } => Some((**inner).clone()),
                        _ => unreachable!("matched object/dict"),
                    };
                    let simplified = schema
                        .map(|s| s.simplify(item))
                        .unwrap_or_else(|| item.clone());
                    if self.type_name() == "dict" || !simplified.is_nullish() {
                        result.insert(key.clone(), simplified);
                    }
                }
                if Data::deep_equal(
                    &Data::Object(result.clone()),
                    self.meta().default.as_ref().unwrap_or(&Data::Undefined),
                    self.type_name() == "dict",
                ) {
                    return Data::Null;
                }
                Data::Object(result)
            }
            Node::Array(_) | Node::Tuple(_) => {
                let Data::Array(list) = value else {
                    return value.clone();
                };
                let result: Vec<Data> = list
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let schema = match &self.inner.node {
                            Node::Array(inner) => Some((**inner).clone()),
                            Node::Tuple(list) => list.get(index).cloned(),
                            _ => unreachable!("matched array/tuple"),
                        };
                        schema
                            .map(|s| s.simplify(item))
                            .unwrap_or_else(|| item.clone())
                    })
                    .collect();
                Data::Array(result)
            }
            Node::Intersect(list) => {
                let mut result = IndexMap::new();
                for item in list {
                    if let Data::Object(map) = item.simplify(value) {
                        for (key, value) in map {
                            result.insert(key, value);
                        }
                    }
                }
                Data::Object(result)
            }
            Node::Union(list) => {
                for schema in list {
                    if Schema::validate(schema, value.clone()).is_ok() {
                        return schema.simplify(value);
                    }
                }
                value.clone()
            }
            _ => value.clone(),
        }
    }

    /// Clone with descriptions merged from locale messages (TS `i18n`).
    pub fn i18n(&self, messages: &IndexMap<String, Data>) -> Schema {
        let mut schema = self.derive();
        let desc = merge_desc(self.meta().description.as_ref(), messages);
        if !desc.is_empty() {
            let inner = Arc::get_mut(&mut schema.inner).expect("derived schema is unique");
            inner.meta.description = Some(desc);
        }
        let inner = Arc::get_mut(&mut schema.inner).expect("derived schema is unique");
        match &mut inner.node {
            Node::Object(dict) => {
                let children: Vec<(String, Schema)> = dict
                    .iter()
                    .map(|(key, child)| {
                        let sub = child_messages(messages, &|data| {
                            let inner = get_inner(data)?;
                            let map = inner_object(inner)?;
                            map.get(key)
                                .cloned()
                                .or_else(|| inner_object(data)?.get(key).cloned())
                        });
                        (key.clone(), child.i18n(&sub))
                    })
                    .collect();
                dict.clear();
                dict.extend(children);
            }
            Node::Tuple(list) => {
                let children: Vec<Schema> = list
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        let sub = child_messages(messages, &|data| {
                            let inner = get_inner(data)?;
                            inner_array(inner)?
                                .get(index)
                                .cloned()
                                .or_else(|| inner_array(data)?.get(index).cloned())
                        });
                        child.i18n(&sub)
                    })
                    .collect();
                *list = children;
            }
            Node::Array(child) | Node::Dict { inner: child, .. } => {
                let sub = child_messages(messages, &|data| {
                    if let Some(inner) = get_inner(data) {
                        return Some(inner.clone());
                    }
                    Some(Data::Object(extract_keys(data)))
                });
                let updated = child.i18n(&sub);
                **child = updated;
            }
            _ => {}
        }
        if let Node::Dict { s_key, .. } = &mut inner.node {
            let sub = child_messages(messages, &|data| inner_object(data)?.get("$key").cloned());
            **s_key = s_key.i18n(&sub);
        }
        schema
    }

    /// Format this schema as a compact TypeScript-like type string
    /// (TS `toString(inline?)`).
    pub fn type_string(&self, inline: bool) -> String {
        match &self.inner.node {
            Node::Any => "any".to_string(),
            Node::Never => "never".to_string(),
            Node::Const(value) => match value {
                Data::String(text) => serde_json::Value::String(text.clone()).to_string(),
                other => other.to_js_string(),
            },
            Node::String => "string".to_string(),
            Node::Number => "number".to_string(),
            Node::Boolean => "boolean".to_string(),
            Node::Bitset(_) => "bitset".to_string(),
            Node::Function => "function".to_string(),
            Node::Is(name) => name.clone(),
            Node::Array(inner) => format!("{}[]", inner.type_string(true)),
            Node::Dict { inner, s_key } => {
                format!(
                    "{{ [key: {}]: {} }}",
                    s_key.type_string(false),
                    inner.type_string(false)
                )
            }
            Node::Tuple(list) => format!(
                "[{}]",
                list.iter()
                    .map(|inner| inner.type_string(false))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Node::Object(dict) => {
                if dict.is_empty() {
                    return "{}".to_string();
                }
                format!(
                    "{{ {} }}",
                    dict.iter()
                        .map(|(key, inner)| {
                            format!(
                                "{}{}: {}",
                                key,
                                if inner.meta().required { "" } else { "?" },
                                inner.type_string(false)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Node::Union(list) => {
                let result = list
                    .iter()
                    .map(|inner| inner.type_string(false))
                    .collect::<Vec<_>>()
                    .join(" | ");
                if inline {
                    format!("({result})")
                } else {
                    result
                }
            }
            Node::Intersect(list) => list
                .iter()
                .map(|inner| inner.type_string(true))
                .collect::<Vec<_>>()
                .join(" & "),
            Node::Transform { inner, .. } => inner.type_string(inline),
            Node::Lazy { .. } => "lazy".to_string(),
        }
    }
}

impl fmt::Display for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_string(false))
    }
}

/// Standard Schema V1 `~standard.validate` (TS `schema['~standard']`).
pub trait StandardSchemaV1 {
    fn standard_validate(&self, value: Data) -> StandardResult;
}

impl StandardSchemaV1 for Schema {
    fn standard_validate(&self, value: Data) -> StandardResult {
        let mut data = value;
        match Schema::resolve(&mut data, self, &Options::default(), false) {
            Ok(value) => StandardResult::Success { value },
            Err(error) => StandardResult::Failure {
                issues: vec![Issue {
                    message: error.message.clone(),
                    path: error.path.clone(),
                }],
            },
        }
    }
}

// ---- node resolvers ----

fn resolve_node(
    data: &mut Data,
    schema: &Schema,
    options: &Options,
    strict: bool,
) -> Result<Data, ValidationError> {
    match &schema.inner.node {
        Node::Any => Ok(data.clone()),
        Node::Never => Err(ValidationError::new(
            format!("expected nullable but got {}", data.to_js_string()),
            options,
        )),
        Node::Const(value) => {
            if Data::deep_equal(data, value, false) {
                Ok(value.clone())
            } else {
                Err(ValidationError::new(
                    format!(
                        "expected {} but got {}",
                        value.to_js_string(),
                        data.to_js_string()
                    ),
                    options,
                ))
            }
        }
        Node::String => resolve_string(data, schema, options),
        Node::Number => resolve_number(data, schema, options),
        Node::Boolean => match data {
            Data::Bool(_) => Ok(data.clone()),
            _ => Err(ValidationError::new(
                format!("expected boolean but got {}", data.to_js_string()),
                options,
            )),
        },
        Node::Bitset(bits) => resolve_bitset(data, bits, schema, options),
        Node::Function => match data {
            Data::Instance { .. } => Ok(data.clone()),
            _ => Err(ValidationError::new(
                format!("expected function but got {}", data.to_js_string()),
                options,
            )),
        },
        Node::Is(name) => {
            let matched = match data {
                Data::Instance {
                    name: instance_name,
                    ..
                } => instance_name == name,
                Data::Date(_) => name == "Date",
                Data::RegExp { .. } => name == "RegExp",
                Data::Binary(_) => name == "ArrayBuffer" || name == "SharedArrayBuffer",
                _ => false,
            };
            if matched {
                Ok(data.clone())
            } else {
                Err(ValidationError::new(
                    format!("expected {name} but got {}", data.to_js_string()),
                    options,
                ))
            }
        }
        Node::Array(inner) => {
            let Data::Array(list) = data else {
                return Err(ValidationError::new(
                    format!("expected array but got {}", data.to_js_string()),
                    options,
                ));
            };
            let skip_min = inner
                .inner
                .meta
                .default
                .as_ref()
                .is_some_and(|d| !d.is_nullish());
            check_within_range(
                list.len() as f64,
                &schema.inner.meta,
                "array length",
                options,
                skip_min,
            )?;
            let mut result = Vec::with_capacity(list.len());
            for index in 0..list.len() {
                result.push(property(data, &PathKey::Index(index), inner, options)?);
            }
            Ok(Data::Array(result))
        }
        Node::Dict { inner, s_key } => {
            let Data::Object(map) = data else {
                return Err(ValidationError::new(
                    format!("expected object but got {}", data.to_js_string()),
                    options,
                ));
            };
            let keys: Vec<String> = map.keys().cloned().collect();
            let mut result = IndexMap::new();
            for key in keys {
                let mut key_data = Data::String(key.clone());
                let r_key = match Schema::resolve(&mut key_data, s_key, options, false) {
                    Ok(value) => match value {
                        Data::String(text) => text,
                        _ => key.clone(),
                    },
                    Err(error) => {
                        if strict {
                            continue;
                        }
                        return Err(error);
                    }
                };
                let value = property(data, &PathKey::Key(key.clone()), inner, options)?;
                result.insert(r_key.clone(), value);
                let moved = data
                    .member_mut(&PathKey::Key(key.clone()))
                    .cloned()
                    .unwrap_or(Data::Undefined);
                data.set_member(PathKey::Key(r_key.clone()), moved);
                if r_key != key {
                    data.remove_member(&PathKey::Key(key));
                }
            }
            Ok(Data::Object(result))
        }
        Node::Tuple(list) => {
            let Data::Array(data_list) = data else {
                return Err(ValidationError::new(
                    format!("expected array but got {}", data.to_js_string()),
                    options,
                ));
            };
            let rest: Vec<Data> = if strict {
                Vec::new()
            } else {
                data_list.iter().skip(list.len()).cloned().collect()
            };
            let mut result = Vec::with_capacity(list.len());
            for (index, inner) in list.iter().enumerate() {
                result.push(property(data, &PathKey::Index(index), inner, options)?);
            }
            result.extend(rest);
            Ok(Data::Array(result))
        }
        Node::Object(dict) => {
            let Data::Object(_) = data else {
                return Err(ValidationError::new(
                    format!("expected object but got {}", data.to_js_string()),
                    options,
                ));
            };
            let mut result = IndexMap::new();
            for (key, schema) in dict {
                let key_path = PathKey::Key(key.clone());
                let value = property(data, &key_path, schema, options)?;
                if !value.is_nullish() || data.has_member(&key_path) {
                    result.insert(key.clone(), value);
                }
            }
            if !strict && let Data::Object(map) = data {
                merge_object_maps(&mut result, map);
            }
            Ok(Data::Object(result))
        }
        Node::Union(list) => {
            for inner in list {
                if let Ok(value) = Schema::resolve(data, inner, options, strict) {
                    return Ok(value);
                }
            }
            Err(ValidationError::new(
                format!(
                    "expected {} but got {}",
                    schema.type_string(false),
                    data.to_json_string()
                ),
                options,
            ))
        }
        Node::Intersect(list) => {
            if list.is_empty() {
                return Ok(data.clone());
            }
            let mut result: Option<Data> = None;
            for inner in list {
                let value = Schema::resolve(data, inner, options, true)?;
                if value.is_nullish() {
                    continue;
                }
                match &mut result {
                    None => result = Some(value),
                    Some(current) => {
                        if std::mem::discriminant(current) != std::mem::discriminant(&value) {
                            return Err(ValidationError::new(
                                format!(
                                    "expected {} but got {}",
                                    schema.type_string(false),
                                    data.to_json_string()
                                ),
                                options,
                            ));
                        }
                        match (current, &value) {
                            (Data::Object(target), Data::Object(source)) => {
                                merge_object_maps(target, source);
                            }
                            (Data::Array(_), Data::Array(_)) => {
                                // JS `merge` walks index keys; arrays are
                                // merged index-wise in TS. Skipped here
                                // (documented deviation).
                            }
                            (current, value) => {
                                if !Data::deep_equal(current, value, true) {
                                    return Err(ValidationError::new(
                                        format!(
                                            "expected {} but got {}",
                                            schema.type_string(false),
                                            data.to_json_string()
                                        ),
                                        options,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if !strict && data.is_object() {
                match &mut result {
                    None => result = Some(data.clone()),
                    Some(current) => merge(current, data),
                }
            }
            Ok(result.unwrap_or_else(|| data.clone()))
        }
        Node::Transform {
            inner,
            callback,
            preserve,
        } => {
            let result = Schema::resolve(data, inner, options, true)?;
            let adapted = data.clone();
            if *preserve {
                callback(&result, options)
            } else {
                let value = callback(&result, options)?;
                let adapted = callback(&adapted, options)?;
                *data = adapted;
                Ok(value)
            }
        }
        Node::Lazy { builder, cache } => {
            let inner: Schema = {
                let mut guard = cache.lock();
                if guard.is_none() {
                    let mut built = builder();
                    // merge meta: built.meta = { ...schema.meta, ...built.meta }
                    let built_inner =
                        Arc::get_mut(&mut built.inner).expect("fresh schema is unique");
                    built_inner.meta = {
                        let mut meta = schema.inner.meta.clone();
                        merge_meta(&mut meta, &built_inner.meta);
                        meta
                    };
                    *guard = Some(Box::new(built));
                }
                guard.as_ref().unwrap().as_ref().clone()
            };
            Schema::resolve(data, &inner, options, strict)
        }
    }
}

/// TS: built meta merge `{ ...schema.meta, ...inner.meta }` — inner wins.
fn merge_meta(target: &mut Meta, overlay: &Meta) {
    if let Some(value) = &overlay.default {
        target.default = Some(value.clone());
    }
    if overlay.required {
        target.required = true;
    }
    if overlay.disabled {
        target.disabled = true;
    }
    if overlay.collapse {
        target.collapse = true;
    }
    if overlay.hidden {
        target.hidden = true;
    }
    if overlay.loose {
        target.loose = true;
    }
    if !overlay.badges.is_empty() {
        target.badges.extend(overlay.badges.iter().cloned());
    }
    if let Some(role) = &overlay.role {
        target.role = Some(role.clone());
    }
    if let Some(extra) = &overlay.extra {
        target.extra = Some(extra.clone());
    }
    if let Some(link) = &overlay.link {
        target.link = Some(link.clone());
    }
    if let Some(description) = &overlay.description {
        target.description = Some(description.clone());
    }
    if let Some(comment) = &overlay.comment {
        target.comment = Some(comment.clone());
    }
    if let Some(pattern) = &overlay.pattern {
        target.pattern = Some(pattern.clone());
    }
    if let Some(max) = overlay.max {
        target.max = Some(max);
    }
    if let Some(min) = overlay.min {
        target.min = Some(min);
    }
    if let Some(step) = overlay.step {
        target.step = Some(step);
    }
}

fn resolve_string(
    data: &mut Data,
    schema: &Schema,
    options: &Options,
) -> Result<Data, ValidationError> {
    let Data::String(text) = data else {
        return Err(ValidationError::new(
            format!("expected string but got {}", data.to_js_string()),
            options,
        ));
    };
    if let Some(pattern) = &schema.inner.meta.pattern {
        let Some(regex) = build_regex(&pattern.source, pattern.flags.as_deref().unwrap_or(""))
        else {
            return Err(ValidationError::new(
                format!("invalid regexp \"{}\"", pattern.source),
                options,
            ));
        };
        if !regex.is_match(text) {
            return Err(ValidationError::new(
                format!("expect string to match regexp {regex}"),
                options,
            ));
        }
    }
    check_within_range(
        text.chars().count() as f64,
        &schema.inner.meta,
        "string length",
        options,
        false,
    )?;
    Ok(data.clone())
}

fn resolve_number(
    data: &mut Data,
    schema: &Schema,
    options: &Options,
) -> Result<Data, ValidationError> {
    let Data::Number(value) = data else {
        return Err(ValidationError::new(
            format!("expected number but got {}", data.to_js_string()),
            options,
        ));
    };
    let value = *value;
    check_within_range(value, &schema.inner.meta, "number", options, false)?;
    if let Some(step) = schema.inner.meta.step
        && step != 0.0
        && !is_multiple_of(value, schema.inner.meta.min.unwrap_or(0.0), step)
    {
        return Err(ValidationError::new(
            format!(
                "expected number multiple of {} but got {}",
                js_number_string(step),
                js_number_string(value)
            ),
            options,
        ));
    }
    Ok(Data::Number(value))
}

fn resolve_bitset(
    data: &mut Data,
    bits: &IndexMap<String, u64>,
    schema: &Schema,
    options: &Options,
) -> Result<Data, ValidationError> {
    let mut value: u64 = 0;
    let mut keys: Vec<String> = Vec::new();
    match data {
        Data::Number(number) => {
            value = *number as u64;
            for key in bits.keys() {
                if value & bits[key] != 0 {
                    keys.push(key.clone());
                }
            }
        }
        Data::Array(list) => {
            for item in list {
                let Data::String(key) = item else {
                    return Err(ValidationError::new(
                        format!("expected string but got {}", item.to_js_string()),
                        options,
                    ));
                };
                keys.push(key.clone());
                if let Some(bit) = bits.get(key) {
                    value |= bit;
                }
            }
        }
        _ => {
            return Err(ValidationError::new(
                format!("expected number or array but got {}", data.to_js_string()),
                options,
            ));
        }
    }
    let default_value = match schema.inner.meta.default.as_ref() {
        Some(Data::Number(number)) => *number as u64,
        _ => 0,
    };
    if value == default_value {
        return Ok(Data::Number(value as f64));
    }
    // TS returns `[value, keys]`: the input is adapted to the key array.
    *data = Data::Array(keys.iter().map(|key| Data::String(key.clone())).collect());
    Ok(Data::Number(value as f64))
}

fn check_within_range(
    data: f64,
    meta: &Meta,
    description: &str,
    options: &Options,
    skip_min: bool,
) -> Result<(), ValidationError> {
    let max = meta.max.unwrap_or(f64::INFINITY);
    let min = meta.min.unwrap_or(f64::NEG_INFINITY);
    if data > max {
        return Err(ValidationError::new(
            format!(
                "expected {description} <= {} but got {}",
                js_number_string(max),
                js_number_string(data)
            ),
            options,
        ));
    }
    if data < min && !skip_min {
        return Err(ValidationError::new(
            format!(
                "expected {description} >= {} but got {}",
                js_number_string(min),
                js_number_string(data)
            ),
            options,
        ));
    }
    Ok(())
}

/// TS `decimalShift`: move the decimal point right by `digits` positions.
fn decimal_shift(data: f64, digits: usize) -> f64 {
    let text = format!("{data}");
    if text.contains('e') || text.contains('E') {
        return data * 10f64.powi(digits as i32);
    }
    match text.find('.') {
        None => data * 10f64.powi(digits as i32),
        Some(index) => {
            let frac = &text[index + 1..];
            let integer = &text[..index];
            if frac.len() <= digits {
                format!("{integer}{frac}{}", "0".repeat(digits - frac.len()))
                    .parse()
                    .unwrap_or(f64::NAN)
            } else {
                format!("{integer}{}.{}", &frac[..digits], &frac[digits..])
                    .parse()
                    .unwrap_or(f64::NAN)
            }
        }
    }
}

/// TS `isMultipleOf`: exact modulo for integer steps, decimal-shift modulo
/// otherwise.
fn is_multiple_of(data: f64, min: f64, step: f64) -> bool {
    let step = step.abs();
    let step_text = format!("{step}");
    let is_decimal = step_text.split_once('.').is_some_and(|(integer, frac)| {
        !integer.is_empty()
            && !frac.is_empty()
            && integer.chars().all(|c| c.is_ascii_digit())
            && frac.chars().all(|c| c.is_ascii_digit())
    });
    if !is_decimal {
        return (data - min) % step == 0.0;
    }
    let index = step_text.find('.').unwrap();
    let digits = step_text[index + 1..].len();
    (decimal_shift(data, digits) - decimal_shift(min, digits)).abs() % decimal_shift(step, digits)
        == 0.0
}

/// Resolve one member of an array/object input with adaptation write-back
/// (TS `property()`).
fn property(
    data: &mut Data,
    key: &PathKey,
    schema: &Schema,
    options: &Options,
) -> Result<Data, ValidationError> {
    let segment = match key {
        PathKey::Key(key) => PathSeg::Key(key.clone()),
        PathKey::Index(index) => PathSeg::Index(*index),
    };
    let options = options.with_path(segment);
    let result = match data.member_mut(key) {
        Some(member) => Schema::resolve(member, schema, &options, false),
        None => {
            let mut slot = Data::Undefined;
            Schema::resolve(&mut slot, schema, &options, false)
        }
    };
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if options.autofix {
                data.remove_member(key);
                Ok(schema.inner.meta.default.clone().unwrap_or(Data::Undefined))
            } else {
                Err(error)
            }
        }
    }
}

/// JS `merge`: copy keys from `source` that `result` does not have.
fn merge(result: &mut Data, source: &Data) {
    if let (Data::Object(target), Data::Object(source)) = (result, source) {
        merge_object_maps(target, source);
    }
}

fn merge_object_maps(target: &mut IndexMap<String, Data>, source: &IndexMap<String, Data>) {
    for (key, value) in source {
        if !target.contains_key(key) {
            target.insert(key.clone(), value.clone());
        }
    }
}

/// Build a regex from a JS pattern source + flags (translated `i/m/s`).
pub(crate) fn build_regex(source: &str, flags: &str) -> Option<Regex> {
    let mut prefix = String::from("(?");
    if flags.contains('i') {
        prefix.push('i');
    }
    if flags.contains('m') {
        prefix.push('m');
    }
    if flags.contains('s') {
        prefix.push('s');
    }
    if prefix.len() > 2 {
        prefix.push(')');
        Regex::new(&format!("{prefix}{source}")).ok()
    } else {
        Regex::new(source).ok()
    }
}

// ---- i18n helpers ----

fn get_inner(data: &Data) -> Option<&Data> {
    let object = inner_object(data)?;
    object.get("$value").or_else(|| object.get("$inner"))
}

fn inner_object(data: &Data) -> Option<&IndexMap<String, Data>> {
    match data {
        Data::Object(map) => Some(map),
        _ => None,
    }
}

fn inner_array(data: &Data) -> Option<&Vec<Data>> {
    match data {
        Data::Array(list) => Some(list),
        _ => None,
    }
}

fn extract_keys(data: &Data) -> IndexMap<String, Data> {
    match data {
        Data::Object(map) => map
            .iter()
            .filter(|(key, _)| !key.starts_with('$'))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => IndexMap::new(),
    }
}

fn child_messages(
    messages: &IndexMap<String, Data>,
    pick: &dyn Fn(&Data) -> Option<Data>,
) -> IndexMap<String, Data> {
    messages
        .iter()
        .filter_map(|(key, data)| pick(data).map(|value| (key.clone(), value)))
        .collect()
}

fn merge_desc(original: Option<&Desc>, messages: &IndexMap<String, Data>) -> Desc {
    let mut result = match original {
        None => IndexMap::new(),
        Some(Desc::Plain(text)) => {
            let mut map = IndexMap::new();
            map.insert(String::new(), text.clone());
            map
        }
        Some(Desc::Localized(map)) => map.clone(),
    };
    for (locale, value) in messages {
        if let Data::Object(map) = value {
            if let Some(Data::String(text)) = map.get("$description") {
                result.insert(locale.clone(), text.clone());
            } else if let Some(Data::String(text)) = map.get("$desc") {
                result.insert(locale.clone(), text.clone());
            }
        } else if let Data::String(text) = value {
            result.insert(locale.clone(), text.clone());
        }
    }
    Desc::Localized(result)
}
