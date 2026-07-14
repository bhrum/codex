use fabushi_telegram_wasm::TelegramWasmClient;
use serde_json::json;
use serde_json::Value;

fn execute(client: &mut TelegramWasmClient, request: Value) -> Value {
    serde_json::from_str(&client.execute_json(&request.to_string())).unwrap()
}

#[test]
fn wasm_bridge_uses_the_same_core_contract_and_round_trips_state() {
    let mut first = TelegramWasmClient::new();
    let status = execute(
        &mut first,
        json!({"@type": "telegram.getStatus", "@extra": "web-1"}),
    );
    assert_eq!(status["data"]["platform"], "web");
    assert_eq!(status["data"]["@extra"], "web-1");
    assert_eq!(status["data"]["persistentStorage"], false);

    let result = execute(
        &mut first,
        json!({
            "@type": "telegram.executeCoreCommand",
            "command": {
                "type": "upsertChat",
                "chat": {
                    "id": 91,
                    "kind": "channel",
                    "title": "Web Rust 会话",
                    "lastMessageId": null,
                    "lastReadInboxMessageId": null,
                    "lastReadOutboxMessageId": null,
                    "unreadCount": 0,
                    "pinnedMessageId": null,
                    "notificationSettings": {
                        "muteUntilUnixMs": null,
                        "soundId": null,
                        "showPreview": true
                    },
                    "isArchived": false
                }
            }
        }),
    );
    assert_eq!(result["ok"], true);

    let snapshot = first.export_state();
    let mut restored = TelegramWasmClient::new();
    let imported: Value = serde_json::from_str(&restored.import_state(&snapshot)).unwrap();
    assert_eq!(imported["ok"], true);
    let state = execute(&mut restored, json!({"@type": "telegram.getState"}));
    assert_eq!(
        state["data"]["state"]["chats"]["91"]["title"],
        "Web Rust 会话"
    );
}

#[test]
fn wasm_bridge_returns_structured_errors_instead_of_throwing() {
    let mut client = TelegramWasmClient::new();
    let response: Value = serde_json::from_str(&client.execute_json("not-json")).unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["errorCode"], "invalid_json");
}
