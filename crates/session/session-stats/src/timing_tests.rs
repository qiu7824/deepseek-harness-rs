use crate::session_stats_projection_definition;
use serde_json::{Value, json};

#[test]
fn cancelled_failed_and_legacy_requests_contribute_without_double_counting() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../tests/fixtures/request-timing.json")).unwrap();
    let definition = session_stats_projection_definition();
    let header = serde_json::from_value(json!({"version":dsh_session::SESSION_FORMAT_VERSION,"id":"timing-test","createdAt":0,"isSeeded":false})).unwrap();
    for case in cases {
        let mut state = (definition.init)(&header);
        for event in case["events"].as_array().unwrap() {
            let event = serde_json::from_value(event.clone()).unwrap();
            state = (definition.apply)(&state, &event);
        }
        let view = (definition.view)(&state);
        let value: &Value = cordis::downcast(&view).unwrap();
        for (field, expected) in case["expected"].as_object().unwrap() {
            assert_eq!(&value[field], expected, "{}: {field}", case["name"]);
        }
        (definition.schema)(&view).unwrap();
    }
}
