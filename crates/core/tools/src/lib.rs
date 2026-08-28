//! Tool argument schema DSL, enforced JSON Schema subset, registry, and
//! pre/guard/around/post/result execution pipeline: Rust port of
//! `@deepseek-ai/dsh-tools` (native presentation mode; the Code Mode
//! transport and SDK renderers arrive with the dsh-code-runtime
//! milestone).

mod code_mode;
pub mod index;
pub mod json_schema;
pub mod presentation;
pub mod schema;
mod security_policy;
pub mod types;

pub use index::{
    AbortPredicate, Config, DispatchOutcome, PostToolDecision, PreToolDecision, Preparation,
    RUN_CODE_NAME, TOOL_ABORTED, TOOL_ABORTED_BEFORE_DISPATCH, ToolBodyError, ToolDefinition,
    ToolErrorInfo, ToolExecution, ToolExecutionInput, ToolExecutionMode, ToolExecutionResult,
    ToolFailure, ToolGuard, ToolNotFoundError, ToolOutputDefinition, ToolOutputError,
    ToolPresentationMode, ToolRestriction, ToolRunContext, ToolRuntime,
};
pub use json_schema::{
    JsonSchemaError, JsonSchemaNode, ObjectJsonSchema, assert_object_json_schema,
    assert_supported_json_schema, validate_json_schema_value,
};
pub use presentation::{
    FileDiff, FileLocation, ReadFileLine, ReadResultView, SearchFileMatches, SearchLineMatch,
    SearchResultView, ToolCallKind, ToolCallView, ToolResult, ToolResultView, WebResultView,
    WebSource,
};
pub use schema::{
    ArrayValueSchemaSpec, BooleanValueSchemaSpec, IntegerValueSchemaSpec, JsonValueSchemaSpec,
    NullValueSchemaSpec, NumberValueSchemaSpec, ObjectValueSchemaSpec, OneOfValueSchemaSpec,
    ParameterJsonSchema, ParameterPropertySpec, ParameterSchemaSpec, StringValueSchemaSpec,
    ToolArgsError, ValueSchemaAnnotations, ValueSchemaSpec, parameter_schema_spec_to_json_schema,
    validate_args, value_schema_spec_to_json_schema,
};
pub use types::{CodeDispatchEventData, CodeDispatchStartEventData};

pub fn install_security_policy(ctx: &cordis::Context) {
    security_policy::install(ctx);
}
