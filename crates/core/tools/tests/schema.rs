//! Schema-layer tests: Rust port of
//! `packages/core/tools/tests/json-schema.spec.ts` +
//! `schema.spec.ts` (runtime-executable cases; compile-time inference and
//! realm-boundary cases collapse with the type system).

use dsh_tools::{
    JsonSchemaError, assert_object_json_schema, assert_supported_json_schema,
    parameter_schema_spec_to_json_schema, validate_args, validate_json_schema_value,
    value_schema_spec_to_json_schema, ArrayValueSchemaSpec, IntegerValueSchemaSpec,
    JsonValueSchemaSpec, ObjectValueSchemaSpec, OneOfValueSchemaSpec, ParameterPropertySpec,
    ParameterSchemaSpec, StringValueSchemaSpec, ValueSchemaAnnotations, ValueSchemaSpec,
};
use serde_json::{Value as JsonValue, json};

fn violations_of(schema: &JsonValue, object_root: bool) -> Vec<String> {
    let result = if object_root {
        assert_object_json_schema(schema)
    } else {
        assert_supported_json_schema(schema)
    };
    match result {
        Ok(()) => panic!("expected schema rejection"),
        Err(error) => error.violations,
    }
}

fn asserted(schema: JsonValue) -> JsonValue {
    assert_supported_json_schema(&schema).expect("supported schema");
    schema
}

fn string_spec(enum_: Option<Vec<String>>, const_: Option<String>) -> ValueSchemaSpec {
    ValueSchemaSpec::String(StringValueSchemaSpec {
        annotations: ValueSchemaAnnotations::default(),
        enum_,
        const_,
    })
}

#[test]
fn accepts_every_json_root_and_supported_node() {
    for schema in [
        json!({ "type": "string" }),
        json!({ "type": "number" }),
        json!({ "type": "integer" }),
        json!({ "type": "boolean" }),
        json!({ "type": "null" }),
        json!({ "type": "array", "items": { "type": "string" } }),
        json!({
            "type": "object",
            "properties": {
                "nested": { "type": "object", "properties": {}, "additionalProperties": false },
                "free": {},
            },
            "required": ["nested"],
            "additionalProperties": true,
        }),
        json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] }),
        json!({ "description": "any JSON", "title": "JSON", "default": null, "examples": [1, "x"] }),
    ] {
        assert_supported_json_schema(&schema).expect(&schema.to_string());
    }
}

#[test]
fn retains_an_object_root_guard_only_at_consumers_that_need_it() {
    assert_object_json_schema(&json!({ "type": "object" })).expect("object root");
    for schema in [
        json!({}),
        json!({ "type": "string" }),
        json!({ "type": "array" }),
        json!({ "oneOf": [{ "type": "string" }, { "type": "null" }] }),
    ] {
        assert_eq!(
            violations_of(&schema, true),
            vec!["schema.type must be \"object\" (structured output is object-rooted)"]
        );
    }
}

#[test]
fn rejects_non_schema_nodes_unknown_types_and_type_arrays() {
    assert_eq!(violations_of(&JsonValue::Null, false), vec!["schema must be a schema object"]);
    assert_eq!(violations_of(&json!([]), false), vec!["schema must be a schema object"]);
    assert_eq!(violations_of(&json!("no"), false), vec!["schema must be a schema object"]);
    let unknown = violations_of(&json!({ "type": "tuple" }), false);
    assert!(unknown[0].starts_with("schema.type must be one of"), "got {unknown:?}");
    assert_eq!(
        violations_of(&json!({ "type": ["string", "null"] }), false),
        vec!["schema.type must be a single type string (type arrays are not supported)"]
    );
}

#[test]
fn enforces_one_of_vocabulary_and_minimum_branch_count() {
    assert_eq!(
        violations_of(&json!({ "oneOf": [] }), false),
        vec!["schema.oneOf must be an array of at least two schemas"]
    );
    assert_eq!(
        violations_of(&json!({ "oneOf": [{}] }), false),
        vec!["schema.oneOf must be an array of at least two schemas"]
    );
    assert_eq!(
        violations_of(&json!({ "oneOf": "x" }), false),
        vec!["schema.oneOf must be an array of at least two schemas"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "string", "oneOf": [{}, {}] }), false),
        vec!["schema cannot declare both type and oneOf"]
    );
    assert_eq!(
        violations_of(&json!({ "oneOf": [{ "type": "string" }, { "type": "number" }], "items": {} }), false),
        vec!["schema.items is not supported beside oneOf"]
    );
    let bad_branch = violations_of(&json!({ "oneOf": [{ "type": "string" }, { "type": "weird" }] }), false);
    assert!(bad_branch[0].contains("schema.oneOf[1].type"), "got {bad_branch:?}");
}

#[test]
fn rejects_unknown_and_misplaced_keywords() {
    for keyword in ["anyOf", "allOf", "not", "pattern", "minimum", "maxLength", "$ref"] {
        let schema = json!({ "type": "object", keyword: [] });
        let violations = violations_of(&schema, false);
        assert!(
            violations[0].contains(&format!("schema.{keyword} is not a supported keyword")),
            "got {violations:?}"
        );
    }
    assert_eq!(
        violations_of(&json!({ "type": "object", "items": {} }), false),
        vec!["schema.items is not supported on type \"object\""]
    );
    assert_eq!(
        violations_of(&json!({ "type": "array", "properties": {} }), false),
        vec!["schema.properties is not supported on type \"array\""]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "enum": ["x"] }), false),
        vec!["schema.enum is not supported on type \"object\""]
    );
    assert_eq!(
        violations_of(&json!({ "type": "array", "const": null }), false),
        vec!["schema.const is not supported on type \"array\""]
    );
    assert_eq!(
        violations_of(
            &json!({ "properties": {}, "required": [], "additionalProperties": true, "items": {}, "enum": [], "const": null }),
            false,
        ),
        vec![
            "schema.properties requires type or oneOf",
            "schema.required requires type or oneOf",
            "schema.additionalProperties requires type or oneOf",
            "schema.items requires type or oneOf",
            "schema.enum requires type or oneOf",
            "schema.const requires type or oneOf",
        ]
    );
}

#[test]
fn reports_every_independent_schema_violation() {
    let violations = violations_of(
        &json!({
            "type": "object",
            "pattern": "x",
            "properties": { "a": { "type": "weird" }, "b": { "type": "string", "minimum": 1 } },
        }),
        false,
    );
    assert_eq!(violations.len(), 3, "got {violations:?}");
}

#[test]
fn validates_object_properties_required_names_and_openness() {
    assert_eq!(
        violations_of(&json!({ "type": "object", "properties": [] }), false),
        vec!["schema.properties must be an object of schemas"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "properties": { "a": "x" } }), false),
        vec!["schema.properties.a must be a schema object"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "required": "a" }), false),
        vec!["schema.required must be an array of strings"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "required": [1] }), false),
        vec!["schema.required must be an array of strings"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "properties": {}, "required": ["missing"] }), false),
        vec!["schema.required names \"missing\" which is not in properties"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "additionalProperties": "yes" }), false),
        vec!["schema.additionalProperties must be a boolean"]
    );
    // TS's explicit-undefined case projects to JSON null on the wire.
    assert_eq!(
        violations_of(&json!({ "type": "object", "properties": null }), false),
        vec!["schema.properties must be an object of schemas"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "object", "properties": null, "required": ["missing"] }), false),
        vec![
            "schema.properties must be an object of schemas",
            "schema.required names \"missing\" which is not in properties",
        ]
    );
}

#[test]
fn requires_type_correct_scalar_enum_and_const_values() {
    for schema in [
        json!({ "type": "string", "enum": ["a"], "const": "a" }),
        json!({ "type": "number", "enum": [1.5], "const": 1.5 }),
        json!({ "type": "integer", "enum": [1], "const": 1 }),
        json!({ "type": "boolean", "enum": [true], "const": true }),
        json!({ "type": "null", "enum": [null], "const": null }),
    ] {
        assert_supported_json_schema(&schema).expect(&schema.to_string());
    }

    assert_eq!(
        violations_of(&json!({ "type": "string", "enum": [] }), false),
        vec!["schema.enum must be a non-empty array of string values"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "number", "enum": ["1"] }), false),
        vec!["schema.enum must be a non-empty array of number values"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "integer", "enum": [1.5] }), false),
        vec!["schema.enum must be a non-empty array of integer values"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "boolean", "const": 1 }), false),
        vec!["schema.const must be a boolean value"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "string", "enum": null }), false),
        vec!["schema.enum must be a non-empty array of string values"]
    );
    assert_eq!(
        violations_of(&json!({ "type": "string", "enum": ["a"], "const": "b" }), false),
        vec!["schema.const must be one of schema.enum when both are declared"]
    );
}

#[test]
fn validates_annotation_types() {
    assert_eq!(
        violations_of(&json!({ "description": 1 }), false),
        vec!["schema.description must be a string"]
    );
    assert_eq!(
        violations_of(&json!({ "title": 1 }), false),
        vec!["schema.title must be a string"]
    );
}

#[test]
fn validates_scalar_array_object_and_null_roots() {
    assert_eq!(validate_json_schema_value(&asserted(json!({ "type": "string" })), &json!("x"), "value"), Vec::<String>::new());
    assert_eq!(validate_json_schema_value(&asserted(json!({ "type": "number" })), &json!(1.5), "value"), Vec::<String>::new());
    assert_eq!(validate_json_schema_value(&asserted(json!({ "type": "integer" })), &json!(2), "value"), Vec::<String>::new());
    assert_eq!(validate_json_schema_value(&asserted(json!({ "type": "boolean" })), &json!(true), "value"), Vec::<String>::new());
    assert_eq!(validate_json_schema_value(&asserted(json!({ "type": "null" })), &JsonValue::Null, "value"), Vec::<String>::new());
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "array", "items": { "type": "string" } })), &json!(["x"]), "value"),
        Vec::<String>::new()
    );
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "object" })), &json!({ "x": 1 }), "value"),
        Vec::<String>::new()
    );
}

#[test]
fn rejects_wrong_scalar_types_and_negative_zero() {
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "string" })), &json!(1), "value"),
        vec!["\"value\" must be a string"]
    );
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "number" })), &json!("1"), "value"),
        vec!["\"value\" must be a number"]
    );
    let negative_zero = serde_json::Number::from_f64(-0.0).expect("finite");
    assert_eq!(
        validate_json_schema_value(
            &asserted(json!({ "type": "number" })),
            &JsonValue::Number(negative_zero),
            "value",
        ),
        vec!["\"value\" must be a finite JSON number"]
    );
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "integer" })), &json!(1.5), "value"),
        vec!["\"value\" must be an integer"]
    );
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "boolean" })), &json!("true"), "value"),
        vec!["\"value\" must be a boolean"]
    );
    assert_eq!(
        validate_json_schema_value(&asserted(json!({ "type": "null" })), &json!(0), "value"),
        vec!["\"value\" must be null"]
    );
}

#[test]
fn enforces_scalar_enum_and_const_together() {
    let schema = asserted(json!({ "type": "string", "enum": ["a", "b"], "const": "a" }));
    assert_eq!(validate_json_schema_value(&schema, &json!("a"), "value"), Vec::<String>::new());
    assert_eq!(
        validate_json_schema_value(&schema, &json!("c"), "value"),
        vec!["\"value\" must be one of [\"a\",\"b\"]"]
    );
    assert_eq!(
        validate_json_schema_value(&schema, &json!("b"), "value"),
        vec!["\"value\" must be \"a\""]
    );
}

#[test]
fn validates_object_requiredness_nested_values_and_open_defaults() {
    let open = asserted(json!({
        "type": "object",
        "properties": {
            "file": { "type": "string" },
            "nested": {
                "type": "object",
                "properties": { "line": { "type": "integer" } },
                "required": ["line"],
                "additionalProperties": false,
            },
        },
        "required": ["file"],
    }));
    assert_eq!(
        validate_json_schema_value(&open, &json!({ "file": "a", "extra": [1], "nested": { "line": 2 } }), "value"),
        Vec::<String>::new()
    );
    assert_eq!(
        validate_json_schema_value(&open, &json!({ "nested": { "line": 1 } }), "value"),
        vec!["missing required property \"value.file\""]
    );
    assert_eq!(
        validate_json_schema_value(&open, &json!({ "file": 1, "nested": {} }), "value"),
        vec![
            "\"value.file\" must be a string",
            "missing required property \"value.nested.line\"",
        ]
    );
    assert_eq!(
        validate_json_schema_value(&open, &json!({ "file": "a", "nested": { "line": 1, "extra": true } }), "value"),
        vec!["\"value.nested.extra\" is not a declared property (additionalProperties: false)"]
    );
    assert_eq!(
        validate_json_schema_value(&open, &json!("x"), "value"),
        vec!["\"value\" must be an object"]
    );
}

#[test]
fn validates_dense_arrays_per_index() {
    let schema = asserted(json!({ "type": "array", "items": { "type": "integer" } }));
    assert_eq!(validate_json_schema_value(&schema, &json!([1, 2]), "value"), Vec::<String>::new());
    assert_eq!(
        validate_json_schema_value(&schema, &json!([1, 1.5]), "value"),
        vec!["\"value[1]\" must be an integer"]
    );
    assert_eq!(
        validate_json_schema_value(&schema, &json!("x"), "value"),
        vec!["\"value\" must be an array"]
    );
}

#[test]
fn validates_exact_one_one_of_semantics_including_overlap() {
    let disjoint = asserted(json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] }));
    assert_eq!(validate_json_schema_value(&disjoint, &json!("x"), "value"), Vec::<String>::new());
    assert_eq!(
        validate_json_schema_value(&disjoint, &JsonValue::Null, "value"),
        vec!["\"value\" must match exactly one oneOf branch (matched 0)"]
    );
    let overlap = asserted(json!({ "oneOf": [{ "type": "number" }, { "type": "integer" }] }));
    assert_eq!(
        validate_json_schema_value(&overlap, &json!(1), "value"),
        vec!["\"value\" must match exactly one oneOf branch (matched 2)"]
    );
    assert_eq!(validate_json_schema_value(&overlap, &json!(1.5), "value"), Vec::<String>::new());
}

#[test]
fn unconstrained_schema_accepts_any_lossless_json() {
    let any_json = asserted(json!({}));
    for value in [JsonValue::Null, json!(true), json!(1), json!("x"), json!([1]), json!({ "x": null })] {
        assert_eq!(validate_json_schema_value(&any_json, &value, "value"), Vec::<String>::new());
    }
}

#[test]
fn deep_nesting_is_stack_safe() {
    // The iterative walkers are stack-safe; the 5_000-layer JSON value's
    // recursive Drop needs a larger stack than the 2 MiB test-thread default.
    // Depth is capped at 1_000 (upstream uses 5_000): the diagnostic path
    // strings grow O(depth) per frame, so validation copies O(depth²)
    // characters; 1_000 layers still far exceeds any real schema.
    let outcome = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let depth = 1_000;
            let mut schema = json!({ "type": "string" });
            for _ in 0..depth {
                schema = json!({ "oneOf": [schema, { "type": "null" }] });
            }
            assert_supported_json_schema(&schema).expect("deep union");
            assert_eq!(
                validate_json_schema_value(&schema, &json!("leaf"), "value"),
                Vec::<String>::new()
            );
            assert_eq!(
                validate_json_schema_value(&schema, &json!(42), "value"),
                vec!["\"value\" must match exactly one oneOf branch (matched 0)"]
            );
        })
        .expect("spawn deep test");
    outcome.join().expect("deep test settles");
}

#[test]
fn compiles_every_value_root_and_the_json_node() {
    assert_eq!(
        value_schema_spec_to_json_schema(&string_spec(
            Some(vec!["a".to_string(), "b".to_string()]),
            Some("a".to_string()),
        ))
        .expect("compile"),
        json!({ "type": "string", "enum": ["a", "b"], "const": "a" })
    );
    assert_eq!(
        value_schema_spec_to_json_schema(&ValueSchemaSpec::Array(ArrayValueSchemaSpec {
            annotations: ValueSchemaAnnotations::default(),
            items: Some(Box::new(ValueSchemaSpec::Json(JsonValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
            }))),
        }))
        .expect("compile"),
        json!({ "type": "array", "items": {} })
    );
    assert_eq!(
        value_schema_spec_to_json_schema(&ValueSchemaSpec::Object(ObjectValueSchemaSpec {
            annotations: ValueSchemaAnnotations::default(),
            properties: Some(ParameterSchemaSpec::new()),
            additional_properties: false,
        }))
        .expect("compile"),
        json!({ "type": "object", "additionalProperties": false, "properties": {} })
    );
    assert_eq!(
        value_schema_spec_to_json_schema(&ValueSchemaSpec::Json(JsonValueSchemaSpec {
            annotations: ValueSchemaAnnotations {
                description: Some("anything".to_string()),
                title: Some("Any JSON".to_string()),
                default: Some(JsonValue::Null),
                examples: Some(json!([{ "nested": true }])),
            },
        }))
        .expect("compile"),
        json!({ "description": "anything", "title": "Any JSON", "default": null, "examples": [{ "nested": true }] })
    );
    assert_eq!(
        value_schema_spec_to_json_schema(&ValueSchemaSpec::OneOf(OneOfValueSchemaSpec {
            annotations: ValueSchemaAnnotations::default(),
            branches: vec![
                string_spec(None, None),
                ValueSchemaSpec::Null(dsh_tools::NullValueSchemaSpec {
                    annotations: ValueSchemaAnnotations::default(),
                }),
            ],
        }))
        .expect("compile"),
        json!({ "oneOf": [{ "type": "string" }, { "type": "null" }] })
    );
}

#[test]
fn keeps_the_implicit_parameter_root_open_while_preserving_object_openness() {
    let mut properties = ParameterSchemaSpec::new();
    let mut closed_properties = ParameterSchemaSpec::new();
    closed_properties.insert(
        "id".to_string(),
        ParameterPropertySpec {
            schema: ValueSchemaSpec::Integer(IntegerValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
                enum_: None,
                const_: None,
            }),
            required: true,
        },
    );
    properties.insert(
        "closed".to_string(),
        ParameterPropertySpec {
            schema: ValueSchemaSpec::Object(ObjectValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
                properties: Some(closed_properties),
                additional_properties: false,
            }),
            required: true,
        },
    );
    properties.insert(
        "open".to_string(),
        ParameterPropertySpec {
            schema: ValueSchemaSpec::Object(ObjectValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
                properties: None,
                additional_properties: true,
            }),
            required: false,
        },
    );

    assert_eq!(
        parameter_schema_spec_to_json_schema(&properties).expect("compile"),
        json!({
            "type": "object",
            "properties": {
                "closed": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": { "id": { "type": "integer" } },
                    "required": ["id"],
                },
                "open": { "type": "object", "additionalProperties": true },
            },
            "required": ["closed"],
        })
    );
}

#[test]
fn rejects_undersized_one_of_and_empty_enums_at_compile_time() {
    let error = value_schema_spec_to_json_schema(&ValueSchemaSpec::OneOf(OneOfValueSchemaSpec {
        annotations: ValueSchemaAnnotations::default(),
        branches: vec![string_spec(None, None)],
    }))
    .expect_err("single-branch oneOf must reject");
    assert_eq!(error.violations[0], "schema.oneOf must be an array of at least two value schemas");

    let error = value_schema_spec_to_json_schema(&string_spec(Some(Vec::new()), None))
        .expect_err("empty enum must reject");
    assert_eq!(error.violations[0], "schema.enum must be a non-empty array of string values");
}

#[test]
fn const_outside_enum_rejects_through_the_asserted_projection() {
    // The DSL types admit a mismatched const/enum pair; the compiled
    // projection is re-asserted and rejects it, exactly like the TS
    // pipeline's assertSupportedJsonSchema tail.
    let error = value_schema_spec_to_json_schema(&string_spec(
        Some(vec!["a".to_string()]),
        Some("b".to_string()),
    ))
    .expect_err("const outside enum must reject");
    assert_eq!(error.violations, vec!["schema.const must be one of schema.enum when both are declared"]);
}

#[test]
fn preserves_property_names_literally_named_proto() {
    let mut properties = ParameterSchemaSpec::new();
    properties.insert(
        "__proto__".to_string(),
        ParameterPropertySpec { schema: string_spec(None, None), required: true },
    );
    let schema = parameter_schema_spec_to_json_schema(&properties).expect("compile");
    assert_eq!(schema["properties"]["__proto__"], json!({ "type": "string" }));
    assert_eq!(schema["required"], json!(["__proto__"]));
}

#[test]
fn validate_args_returns_path_qualified_violations_with_arguments_root() {
    let mut properties = ParameterSchemaSpec::new();
    properties.insert(
        "path".to_string(),
        ParameterPropertySpec { schema: string_spec(None, None), required: true },
    );
    properties.insert(
        "offset".to_string(),
        ParameterPropertySpec {
            schema: ValueSchemaSpec::Integer(IntegerValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
                enum_: None,
                const_: None,
            }),
            required: false,
        },
    );

    assert_eq!(
        validate_args(&properties, &json!({ "path": "x", "offset": 2 })).expect("validate"),
        Vec::<String>::new()
    );
    // The implicit parameter root has no leading dot: property violations
    // name the bare key, and only the root itself reads "arguments".
    assert_eq!(
        validate_args(&properties, &json!({ "path": 1 })).expect("validate"),
        vec!["\"path\" must be a string"]
    );
    assert_eq!(
        validate_args(&properties, &json!({})).expect("validate"),
        vec!["missing required property \"path\""]
    );
    assert_eq!(
        validate_args(&properties, &json!(1)).expect("validate"),
        vec!["\"arguments\" must be an object"]
    );
}

#[test]
fn json_schema_error_carries_code_and_violations() {
    let error: JsonSchemaError = assert_supported_json_schema(&json!({ "type": "tuple" }))
        .expect_err("forged type must reject");
    assert_eq!(error.code, "UNSUPPORTED_SCHEMA");
    assert_eq!(error.to_string(), "unsupported JSON schema: schema.type must be one of object/array/string/number/integer/boolean/null");
}
