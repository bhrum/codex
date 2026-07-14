use fabushi_telegram_runtime::close_client;
use fabushi_telegram_runtime::create_client;
use fabushi_telegram_runtime::create_persistent_client;
use fabushi_telegram_runtime::execute_json;
use fabushi_telegram_runtime::fabushi_telegram_execute;
use fabushi_telegram_runtime::fabushi_telegram_force_link;
use fabushi_telegram_runtime::fabushi_telegram_free_string;
use serde_json::json;
use serde_json::Value;
use std::ffi::CStr;
use std::ffi::CString;
use tempfile::tempdir;

fn execute(client_id: u64, request: Value) -> Value {
    serde_json::from_str(&execute_json(client_id, &request.to_string())).unwrap()
}

#[test]
fn flutter_friend_chat_payload_is_accepted_without_shape_translation() {
    assert_eq!(fabushi_telegram_force_link(), 1);
    let client = create_client();
    let chat = execute(
        client,
        json!({
            "@type": "telegram.executeCoreCommand",
            "command": {
                "type": "upsertChat",
                "chat": {
                    "id": 1_234_567_890,
                    "kind": "private",
                    "title": "Rust 好友",
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
                    "isArchived": false,
                    "isMarkedUnread": false,
                    "draft": null,
                    "folderIds": []
                }
            }
        }),
    );
    assert_eq!(chat["ok"], true);

    let queued = execute(
        client,
        json!({
            "@type": "telegram.executeCoreCommand",
            "command": {
                "type": "queueMessage",
                "chatId": 1_234_567_890,
                "localMessageId": -1,
                "senderUserId": 1,
                "clientRequestId": "flutter-queue-1",
                "dateUnixMs": 1_720_000_000_000_i64,
                "content": {
                    "type": "text",
                    "data": {"text": "消息进入 Rust 队列", "entities": []}
                },
                "replyToMessageId": null,
                "messageThreadId": null
            }
        }),
    );
    assert_eq!(queued["ok"], true);
    assert_eq!(
        queued["data"]["state"]["messages"][0]["chatId"],
        1_234_567_890
    );
    assert_eq!(
        queued["data"]["state"]["messages"][0]["content"]["data"]["text"],
        "消息进入 Rust 队列"
    );
    assert_eq!(
        queued["data"]["state"]["messages"][0]["deliveryState"]["state"],
        "pending"
    );
    close_client(client).unwrap();
}

#[test]
fn each_client_has_isolated_core_and_authorization_state() {
    let first = create_client();
    let second = create_client();

    let result = execute(
        first,
        json!({
            "@type": "telegram.executeCoreCommand",
            "@extra": "request-1",
            "command": {
                "type": "upsertChat",
                "chat": {
                    "id": 42,
                    "kind": "private",
                    "title": "真实会话",
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
    assert_eq!(result["data"]["@extra"], "request-1");
    assert_eq!(result["data"]["state"]["chats"]["42"]["title"], "真实会话");

    let second_state = execute(second, json!({"@type": "telegram.getState"}));
    assert_eq!(second_state["data"]["state"]["chats"], json!({}));

    let auth = execute(
        first,
        json!({
            "@type": "telegram.executeAuthorizationCommand",
            "command": {"type": "parametersAccepted"}
        }),
    );
    assert_eq!(
        auth["data"]["authorizationState"]["type"],
        "waitPhoneNumber"
    );

    close_client(first).unwrap();
    close_client(second).unwrap();
}

#[test]
fn ffi_boundary_returns_owned_json_and_handles_null_input() {
    let client = create_client();
    let request = CString::new(r#"{"@type":"telegram.getStatus"}"#).unwrap();
    let response_pointer = unsafe { fabushi_telegram_execute(client, request.as_ptr()) };
    let response: Value = unsafe {
        serde_json::from_str(CStr::from_ptr(response_pointer).to_str().unwrap()).unwrap()
    };
    unsafe { fabushi_telegram_free_string(response_pointer) };
    assert_eq!(response["data"]["architecture"], "rust-command-event-core");

    let null_response = unsafe { fabushi_telegram_execute(client, std::ptr::null()) };
    let null_value: Value =
        unsafe { serde_json::from_str(CStr::from_ptr(null_response).to_str().unwrap()).unwrap() };
    unsafe { fabushi_telegram_free_string(null_response) };
    assert_eq!(null_value["errorCode"], "invalid_ffi_request");
    close_client(client).unwrap();
}

#[test]
fn closed_or_unknown_clients_return_structured_errors() {
    let client = create_client();
    close_client(client).unwrap();
    let response = execute(client, json!({"@type": "telegram.getStatus"}));
    assert_eq!(response["ok"], false);
    assert_eq!(response["errorCode"], "client_not_found");
}

#[test]
fn api_connection_initialization_requires_product_configuration_and_transport() {
    let client = create_client();
    let missing = execute(client, json!({"@type": "telegram.initializeConnection"}));
    assert_eq!(missing["errorCode"], "invalid_parameter");

    let disconnected = execute(
        client,
        json!({
            "@type": "telegram.initializeConnection",
            "apiId": 12345,
            "deviceModel": "Fabushi Test",
            "systemVersion": "Rust",
            "appVersion": "0.1.0",
            "systemLangCode": "zh-Hans",
            "langPack": "",
            "langCode": "zh-hans"
        }),
    );
    assert_eq!(disconnected["errorCode"], "transport_not_connected");

    let missing_code_context = execute(
        client,
        json!({
            "@type": "telegram.submitAuthenticationCode",
            "code": "12345"
        }),
    );
    assert_eq!(missing_code_context["errorCode"], "auth_context_missing");
    let missing_password_context = execute(
        client,
        json!({
            "@type": "telegram.submitAuthenticationPassword",
            "password": "secret"
        }),
    );
    assert_eq!(
        missing_password_context["errorCode"],
        "auth_context_missing"
    );
    let sync_before_auth = execute(client, json!({"@type": "telegram.beginUpdateSync"}));
    assert_eq!(
        sync_before_auth["errorCode"],
        "authorization_command_failed"
    );
    close_client(client).unwrap();
}

#[test]
fn persistent_client_restores_committed_core_state() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite3");
    let path_string = path.to_string_lossy();
    let key = [11_u8; 32];
    let client = create_persistent_client(&path_string, &key).unwrap();
    let result = execute(
        client,
        json!({
            "@type": "telegram.executeCoreCommand",
            "command": {
                "type": "upsertChat",
                "chat": {
                    "id": 77,
                    "kind": "savedMessages",
                    "title": "持久化会话",
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
    close_client(client).unwrap();

    let restored = create_persistent_client(&path_string, &key).unwrap();
    let status = execute(restored, json!({"@type": "telegram.getStatus"}));
    assert_eq!(status["data"]["persistentStorage"], true);
    let state = execute(restored, json!({"@type": "telegram.getState"}));
    assert_eq!(state["data"]["state"]["chats"]["77"]["title"], "持久化会话");
    close_client(restored).unwrap();
}

#[test]
#[ignore = "requires a live Telegram production data center"]
fn live_runtime_bootstrap_keeps_an_encrypted_transport_session() {
    let client = create_client();
    let ready = execute(
        client,
        json!({"@type": "telegram.bootstrapTransport", "dcId": 2}),
    );
    assert_eq!(ready["ok"], true, "{ready}");
    assert_eq!(ready["data"]["encryptedPingVerified"], true);
    assert!(ready["data"]["authKeyId"].as_str().is_some());

    let status = execute(client, json!({"@type": "telegram.getStatus"}));
    assert_eq!(status["data"]["transportConnected"], true);
    assert_eq!(status["data"]["dcId"], 2);
    close_client(client).unwrap();
}
