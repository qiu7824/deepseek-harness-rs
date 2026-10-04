use super::*;

#[tokio::test]
async fn describe_carrier_returns_the_same_identity_as_desktop_readiness() {
    let ctx = Context::root();
    let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
    let handler = crate::fetch::handler::to_fetch_handler(service);
    let expected = crate::host_process_identity();
    for _ in 0..2 {
        let response = handler
            .handle(crate::fetch::handler::CarrierRequest {
                method: http::Method::POST,
                path: "/api/host.describe".into(),
                query: vec![],
                headers: vec![("content-type".into(), "application/json".into())],
                body: Some(
                    serde_json::to_vec(&serde_json::json!({
                        "type": "client-request", "rpcId": "host-identity-test",
                        "method": "host.describe", "payload": {}
                    }))
                    .unwrap(),
                ),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("host.describe must return a unary response")
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["result"]["ok"], true, "{value}");
        let description = &value["result"]["value"];
        assert_eq!(description["processId"], expected.0);
        assert_eq!(description["instanceId"], expected.1);
        assert_eq!(description["version"], HOST_VERSION);
    }
}
