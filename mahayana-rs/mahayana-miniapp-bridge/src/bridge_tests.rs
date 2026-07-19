use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

struct TestHost;

impl HostBridge for TestHost {
    fn invoke(&self, method: &str, params: Value) -> Result<Value, BridgeError> {
        Ok(json!({"method": method, "params": params}))
    }
}

#[test]
fn routes_allowed_mcp_calls() {
    let router = BridgeRouter::new(TestHost, [HostPermission::McpCall]);
    let response = router.handle(BridgeRequest::new(
        1,
        method::MCP_CALL_TOOL,
        json!({"tool": "forecast"}),
    ));

    assert_eq!(
        response,
        BridgeResponse {
            jsonrpc: "2.0".into(),
            id: json!(1),
            result: Some(json!({
                "method": "mcp.callTool",
                "params": {"tool": "forecast"}
            })),
            error: None,
        }
    );
}

#[test]
fn rejects_payment_calls_without_permission() {
    let router = BridgeRouter::new(TestHost, []);
    let response = router.handle(BridgeRequest::new(
        "request-1",
        method::COMMERCE_PURCHASE,
        json!({"sku": "weather.pro"}),
    ));

    assert_eq!(response.result, None);
    assert_eq!(response.error.expect("permission error").code, -32001);
}
