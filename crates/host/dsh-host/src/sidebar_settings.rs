//! Durable preferences for the Rust-hosted sidebar's declarative controls.
use cordis::Context;
use dsh_schemastery::{Data, Schema};
use dsh_settings::{SettingsProvider, SettingsRegisterOptions, settings_namespace};
use std::sync::Arc;

pub(crate) fn schema() -> Schema {
    let choice = || {
        Schema::union(vec![
            Schema::constant(Data::String("sidebar".into())),
            Schema::constant(Data::String("external".into())),
        ])
        .default(Data::String("sidebar".into()))
    };
    let mut fields = indexmap::IndexMap::from([
        (
            "width".into(),
            Schema::number()
                .min(420.0)
                .max(1600.0)
                .step(1.0)
                .default(Data::Number(680.0)),
        ),
        (
            "rememberWidth".into(),
            Schema::boolean().default(Data::Bool(true)),
        ),
        (
            "fullscreenOnOpen".into(),
            Schema::boolean().default(Data::Bool(false)),
        ),
        ("httpLinks".into(), choice()),
        ("httpsLinks".into(), choice()),
    ]);
    for field in ["showFiles", "showGit", "showBrowser", "showTerminal"] {
        fields.insert(field.into(), Schema::boolean().default(Data::Bool(true)));
    }
    Schema::object(fields)
}

pub(crate) fn register(ctx: &Context, settings: &Arc<SettingsProvider>) -> Result<(), String> {
    settings
        .register(
            ctx,
            settings_namespace("dsh-better-sidebar").map_err(|error| error.to_string())?,
            schema(),
            SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings dsh-better-sidebar: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_declared_values_are_validated_by_the_host() {
        let schema = schema();
        let defaults = Schema::validate(&schema, Data::Object(Default::default()))
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(defaults["width"].as_f64(), Some(680.0));
        assert_eq!(defaults["rememberWidth"], true);
        assert_eq!(defaults["fullscreenOnOpen"], false);
        for input in [
            serde_json::json!({"width":100}),
            serde_json::json!({"width":2000}),
            serde_json::json!({"httpLinks":"javascript"}),
            serde_json::json!({"showGit":"false"}),
        ] {
            assert!(
                Schema::validate(&schema, super::super::json_to_settings_data(&input)).is_err(),
                "{input}"
            );
        }
        let chosen = Schema::validate(
            &schema,
            super::super::json_to_settings_data(
                &serde_json::json!({"width":960,"showGit":false,"httpsLinks":"external"}),
            ),
        )
        .unwrap()
        .to_json()
        .unwrap();
        assert_eq!(chosen["width"].as_f64(), Some(960.0));
        assert_eq!(chosen["showGit"], false);
        assert_eq!(chosen["httpsLinks"], "external");
    }
}
