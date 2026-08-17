//! Loader config expression helpers (port of `src/config/utils.ts`).

use serde_json::{Map, Value};

/// Serialized JavaScript expression produced by the include YAML `!js` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsExpr {
    pub expr: String,
}

/// Return true when a value is a serialized loader JavaScript expression.
pub fn is_js_expr(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("__jsExpr"))
}

fn js_expr_of(value: &Value) -> Option<JsExpr> {
    value.as_object().and_then(|object| {
        object
            .get("__jsExpr")
            .and_then(|expr| expr.as_str())
            .map(|expr| JsExpr {
                expr: expr.to_string(),
            })
    })
}

/// Recursively replace YAML `!js` expression nodes with evaluated values
/// (TS `interpolate`).
///
/// # Deviation
///
/// Evaluation needs the embedded-JS-runtime milestone; until then any
/// `__jsExpr` node fails with [`InterpolateError`] instead of being passed
/// through raw (which would silently change plugin behavior).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpolateError {
    UnsupportedExpression(String),
}

impl std::fmt::Display for InterpolateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterpolateError::UnsupportedExpression(expr) => write!(
                f,
                "JavaScript expression \"{expr}\" cannot be evaluated yet (embedded JS runtime milestone)"
            ),
        }
    }
}

impl std::error::Error for InterpolateError {}

/// Interpolate a config value, replacing `__jsExpr` nodes (see
/// [`InterpolateError`] for the current limitation).
pub fn interpolate(value: &Value) -> Result<Value, InterpolateError> {
    if let Some(expr) = js_expr_of(value) {
        return Err(InterpolateError::UnsupportedExpression(expr.expr));
    }
    match value {
        Value::Array(items) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(interpolate(item)?);
            }
            Ok(Value::Array(result))
        }
        Value::Object(map) => {
            let mut result = Map::new();
            for (key, item) in map {
                result.insert(key.clone(), interpolate(item)?);
            }
            Ok(Value::Object(result))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn passes_plain_values() {
        let value = json!({"a": 1, "b": [true, "x"], "c": {"d": null}});
        assert_eq!(interpolate(&value).unwrap(), value);
    }

    #[test]
    fn detects_js_exprs() {
        let value = json!({"a": {"__jsExpr": "process.env.FOO"}});
        assert!(is_js_expr(&value["a"]));
        assert!(interpolate(&value).is_err());
        let nested = json!({"b": [{"__jsExpr": "1 + 1"}]});
        assert!(interpolate(&nested).is_err());
    }
}
