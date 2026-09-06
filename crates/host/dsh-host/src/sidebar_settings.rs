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
    let extension_id = || {
        Schema::string()
            .min(1.0)
            .max(96.0)
            .pattern(r"^[A-Za-z0-9][A-Za-z0-9:._-]*$", None)
    };
    let path = || Schema::string().max(8192.0);
    let plugin_tab = Schema::object(indexmap::IndexMap::from([
        (
            "id".into(),
            Schema::string().min(1.0).max(128.0).required(true),
        ),
        ("type".into(), extension_id().required(true)),
        (
            "title".into(),
            Schema::string().min(1.0).max(256.0).required(true),
        ),
        ("path".into(), path()),
        ("diff".into(), Schema::any()),
        ("meta".into(), Schema::any()),
    ]));
    let leaf = || {
        Schema::object(indexmap::IndexMap::from([
            (
                "kind".into(),
                Schema::constant(Data::String("leaf".into())).required(true),
            ),
            (
                "id".into(),
                Schema::string().min(1.0).max(96.0).required(true),
            ),
            ("tabs".into(), Schema::array(plugin_tab.clone()).max(12.0)),
            (
                "active".into(),
                Schema::union(vec![
                    Schema::string().min(1.0).max(128.0),
                    Schema::constant(Data::Null),
                ]),
            ),
        ]))
    };
    let split_with = |child: Schema| {
        Schema::object(indexmap::IndexMap::from([
            (
                "kind".into(),
                Schema::constant(Data::String("split".into())).required(true),
            ),
            (
                "id".into(),
                Schema::string().min(1.0).max(96.0).required(true),
            ),
            (
                "dir".into(),
                Schema::union(vec![
                    Schema::constant(Data::String("row".into())),
                    Schema::constant(Data::String("col".into())),
                ])
                .required(true),
            ),
            (
                "sizes".into(),
                Schema::array(Schema::number().min(0.01).max(1.0))
                    .min(2.0)
                    .max(4.0),
            ),
            ("children".into(), Schema::array(child).min(2.0).max(4.0)),
        ]))
    };
    let depth_one = Schema::union(vec![leaf(), split_with(leaf())]);
    let depth_two = Schema::union(vec![leaf(), split_with(depth_one)]);
    let split_root = Schema::union(vec![leaf(), split_with(depth_two)]);
    let float_window = Schema::object(indexmap::IndexMap::from([
        (
            "id".into(),
            Schema::string().min(1.0).max(96.0).required(true),
        ),
        ("tab".into(), plugin_tab.clone().required(true)),
        ("x".into(), Schema::number().min(0.0).max(10000.0)),
        ("y".into(), Schema::number().min(0.0).max(10000.0)),
        ("w".into(), Schema::number().min(320.0).max(1600.0)),
        ("h".into(), Schema::number().min(200.0).max(1200.0)),
    ]));
    let session_layout = Schema::object(indexmap::IndexMap::from([
        (
            "tab".into(),
            Schema::string()
                .max(128.0)
                .default(Data::String("explorer".into())),
        ),
        ("dir".into(), path().default(Data::String(String::new()))),
        ("file".into(), path().default(Data::String(String::new()))),
        ("files".into(), Schema::array(path()).max(8.0)),
        (
            "url".into(),
            Schema::string()
                .max(8192.0)
                .default(Data::String(String::new())),
        ),
        ("webTabs".into(), Schema::array(path()).max(4.0)),
        ("history".into(), Schema::array(path()).max(20.0)),
        (
            "historyIndex".into(),
            Schema::number()
                .min(-1.0)
                .max(19.0)
                .step(1.0)
                .default(Data::Number(-1.0)),
        ),
        (
            "pluginTabs".into(),
            Schema::array(plugin_tab.clone()).max(12.0),
        ),
        (
            "activePane".into(),
            Schema::string()
                .max(96.0)
                .default(Data::String("right:root".into())),
        ),
        ("splits".into(), split_root.clone()),
        (
            "bottomOpen".into(),
            Schema::boolean().default(Data::Bool(false)),
        ),
        (
            "bottomHeight".into(),
            Schema::number()
                .min(140.0)
                .max(700.0)
                .step(1.0)
                .default(Data::Number(260.0)),
        ),
        ("bottomSplits".into(), split_root),
        ("floats".into(), Schema::array(float_window).max(8.0)),
        (
            "layoutSerial".into(),
            Schema::number()
                .min(1.0)
                .max(999999.0)
                .step(1.0)
                .default(Data::Number(1.0)),
        ),
    ]));
    fields.insert(
        "tabsEnabled".into(),
        Schema::dict(Schema::boolean(), Some(extension_id())),
    );
    fields.insert(
        "viewersEnabled".into(),
        Schema::dict(Schema::boolean(), Some(extension_id())),
    );
    fields.insert(
        "pluginSettings".into(),
        Schema::dict(
            Schema::dict(Schema::any(), Some(Schema::string().max(96.0))),
            Some(extension_id()),
        ),
    );
    fields.insert(
        "sessionLayouts".into(),
        Schema::dict(session_layout, Some(Schema::string().min(1.0).max(256.0))),
    );
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
        assert_eq!(defaults["tabsEnabled"], serde_json::json!({}));
        assert_eq!(defaults["viewersEnabled"], serde_json::json!({}));
        assert_eq!(defaults["pluginSettings"], serde_json::json!({}));
        assert_eq!(defaults["sessionLayouts"], serde_json::json!({}));
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

    #[test]
    fn extension_settings_and_session_layouts_are_bounded_and_typed() {
        let schema = schema();
        let accepted = Schema::validate(
            &schema,
            super::super::json_to_settings_data(&serde_json::json!({
                "tabsEnabled":{"sample:tab":false},
                "viewersEnabled":{"sample:csv":true},
                "pluginSettings":{"sample:tab":{"compact":true,"limit":12}},
                "sessionLayouts":{"session-a":{
                    "tab":"sample:tab:one",
                    "files":["one.rs"],
                    "pluginTabs":[{"id":"sample:tab:one","type":"sample:tab","title":"Sample","meta":{"page":2}}],
                    "activePane":"pane:1",
                    "splits":{"kind":"split","id":"split:1","dir":"row","sizes":[0.5,0.5],"children":[
                        {"kind":"leaf","id":"pane:1","tabs":[{"id":"sample:tab:one","type":"sample:tab","title":"Sample"}],"active":"sample:tab:one"},
                        {"kind":"leaf","id":"pane:2","tabs":[],"active":null}
                    ]},
                    "bottomOpen":true,
                    "bottomHeight":280,
                    "bottomSplits":{"kind":"leaf","id":"pane:3","tabs":[],"active":null},
                    "floats":[{"id":"float:1","tab":{"id":"sample:float","type":"sample:tab","title":"Float"},"x":40,"y":60,"w":480,"h":360}],
                    "layoutSerial":5
                }}
            })),
        )
        .unwrap()
        .to_json()
        .unwrap();
        assert_eq!(accepted["tabsEnabled"]["sample:tab"], false);
        assert_eq!(
            accepted["pluginSettings"]["sample:tab"]["limit"].as_f64(),
            Some(12.0)
        );
        assert_eq!(
            accepted["sessionLayouts"]["session-a"]["pluginTabs"][0]["meta"]["page"].as_f64(),
            Some(2.0)
        );
        assert_eq!(
            accepted["sessionLayouts"]["session-a"]["splits"]["kind"],
            "split"
        );
        assert_eq!(
            accepted["sessionLayouts"]["session-a"]["bottomHeight"].as_f64(),
            Some(280.0)
        );
        assert_eq!(
            accepted["sessionLayouts"]["session-a"]["floats"][0]["w"].as_f64(),
            Some(480.0)
        );

        for rejected in [
            serde_json::json!({"tabsEnabled":{"bad id":true}}),
            serde_json::json!({"viewersEnabled":{"sample:csv":"true"}}),
            serde_json::json!({"pluginSettings":{"sample:tab":[]}}),
            serde_json::json!({"sessionLayouts":{"session-a":{"pluginTabs":[{"id":"x","type":"x"}]}}}),
            serde_json::json!({"sessionLayouts":{"session-a":{"historyIndex":20}}}),
            serde_json::json!({"sessionLayouts":{"session-a":{"splits":{"kind":"split","id":"split:1","dir":"row","sizes":[1],"children":[]}}}}),
            serde_json::json!({"sessionLayouts":{"session-a":{"floats":[{"id":"float:1","tab":{"id":"x","type":"sample:tab","title":"X"},"x":0,"y":0,"w":100,"h":100}]}}}),
        ] {
            assert!(
                Schema::validate(&schema, super::super::json_to_settings_data(&rejected)).is_err(),
                "{rejected}"
            );
        }
    }
}
