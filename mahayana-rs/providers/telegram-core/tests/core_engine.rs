use fabushi_telegram_core::{
    Chat, ChatDraft, ChatFolder, ChatFolderId, ChatId, ChatKind, ClientRequestId, Command,
    DeliveryState, EngineError, FormattedText, Message, MessageContent, MessageId, ReactionCount,
    TelegramEngine, TypingAction, TypingActionKind, UserId, FEATURE_CATALOG,
};
use std::collections::HashSet;

fn engine_with_chat() -> TelegramEngine {
    let mut engine = TelegramEngine::new();
    engine
        .execute(Command::UpsertChat {
            chat: Chat::new(ChatId(7), ChatKind::Private, "测试会话"),
        })
        .unwrap();
    engine
}

fn queue_text(engine: &mut TelegramEngine, request: &str) {
    engine
        .execute(Command::QueueMessage {
            chat_id: ChatId(7),
            local_message_id: MessageId(-1),
            sender_user_id: UserId(9),
            client_request_id: ClientRequestId::new(request),
            date_unix_ms: 100,
            content: MessageContent::Text(FormattedText::plain("南无阿弥陀佛")),
            reply_to_message_id: None,
            message_thread_id: None,
        })
        .unwrap();
}

#[test]
fn outgoing_message_moves_from_local_pending_id_to_server_id() {
    let mut engine = engine_with_chat();
    queue_text(&mut engine, "request-1");

    let pending = &engine.state().messages[&(ChatId(7), MessageId(-1))];
    assert!(matches!(
        pending.delivery_state,
        DeliveryState::Pending { .. }
    ));

    engine
        .execute(Command::AcknowledgeMessage {
            client_request_id: ClientRequestId::new("request-1"),
            server_message_id: MessageId(42),
            date_unix_ms: 200,
        })
        .unwrap();

    assert!(!engine
        .state()
        .messages
        .contains_key(&(ChatId(7), MessageId(-1))));
    let sent = &engine.state().messages[&(ChatId(7), MessageId(42))];
    assert_eq!(sent.delivery_state, DeliveryState::Sent);
    assert_eq!(
        engine.state().chats[&ChatId(7)].last_message_id,
        Some(MessageId(42))
    );
    assert!(engine.state().pending_requests.is_empty());
}

#[test]
fn duplicate_client_request_is_rejected_for_idempotency() {
    let mut engine = engine_with_chat();
    queue_text(&mut engine, "same-request");

    let error = engine
        .execute(Command::QueueMessage {
            chat_id: ChatId(7),
            local_message_id: MessageId(-2),
            sender_user_id: UserId(9),
            client_request_id: ClientRequestId::new("same-request"),
            date_unix_ms: 101,
            content: MessageContent::Text(FormattedText::plain("重复")),
            reply_to_message_id: None,
            message_thread_id: None,
        })
        .unwrap_err();

    assert_eq!(
        error,
        EngineError::DuplicateClientRequest(ClientRequestId::new("same-request"))
    );
}

#[test]
fn edit_pin_read_and_delete_share_one_replayable_state_machine() {
    let mut engine = engine_with_chat();
    queue_text(&mut engine, "request-2");
    engine
        .execute(Command::AcknowledgeMessage {
            client_request_id: ClientRequestId::new("request-2"),
            server_message_id: MessageId(88),
            date_unix_ms: 200,
        })
        .unwrap();
    engine
        .execute(Command::EditMessage {
            chat_id: ChatId(7),
            message_id: MessageId(88),
            content: MessageContent::Text(FormattedText::plain("修改后的消息")),
            edit_date_unix_ms: 300,
        })
        .unwrap();
    engine
        .execute(Command::SetPinnedMessage {
            chat_id: ChatId(7),
            message_id: Some(MessageId(88)),
        })
        .unwrap();
    engine
        .execute(Command::SetChatRead {
            chat_id: ChatId(7),
            last_read_inbox_message_id: MessageId(88),
            unread_count: 0,
        })
        .unwrap();
    engine
        .execute(Command::DeleteMessages {
            chat_id: ChatId(7),
            message_ids: vec![MessageId(88), MessageId(88)],
        })
        .unwrap();

    let chat = &engine.state().chats[&ChatId(7)];
    assert_eq!(chat.pinned_message_id, Some(MessageId(88)));
    assert_eq!(chat.last_read_inbox_message_id, Some(MessageId(88)));
    let message = &engine.state().messages[&(ChatId(7), MessageId(88))];
    assert_eq!(message.edit_date_unix_ms, Some(300));
    assert!(message.is_pinned);
    assert!(message.is_deleted);
}

#[test]
fn command_and_event_contract_is_json_serializable() {
    let command = Command::SetChatRead {
        chat_id: ChatId(7),
        last_read_inbox_message_id: MessageId(5),
        unread_count: 2,
    };
    let json = serde_json::to_value(command).unwrap();
    assert_eq!(json["type"], "setChatRead");
    assert_eq!(json["chatId"], 7);
}

#[test]
fn state_with_messages_round_trips_through_json() {
    let mut engine = engine_with_chat();
    queue_text(&mut engine, "json-round-trip");
    let json = serde_json::to_string(engine.state()).unwrap();
    let restored: fabushi_telegram_core::TelegramState = serde_json::from_str(&json).unwrap();
    assert_eq!(&restored, engine.state());
}

#[test]
fn feature_catalog_has_unique_stable_keys_and_no_false_complete_claims() {
    let mut keys = HashSet::new();
    for feature in FEATURE_CATALOG {
        assert!(keys.insert(feature.key), "duplicate key: {}", feature.key);
        assert_eq!(
            feature.platforms.len(),
            6,
            "{} is missing a platform",
            feature.key
        );
    }
    assert!(FEATURE_CATALOG.len() >= 90);
    assert!(!FEATURE_CATALOG.iter().any(|feature| matches!(
        feature.status,
        fabushi_telegram_core::MigrationStatus::Implemented
    )));
}

#[test]
fn folders_drafts_typing_remote_messages_and_reactions_share_core_state() {
    let mut engine = engine_with_chat();
    engine
        .execute(Command::UpsertChatFolder {
            folder: ChatFolder {
                id: ChatFolderId(3),
                title: "重要".to_string(),
                icon_name: Some("star".to_string()),
                included_chat_ids: vec![ChatId(7)],
                excluded_chat_ids: Vec::new(),
                include_contacts: false,
                include_non_contacts: false,
                include_groups: false,
                include_channels: false,
                include_bots: false,
                exclude_muted: false,
                exclude_read: false,
                exclude_archived: false,
            },
        })
        .unwrap();
    let mut chat = engine.state().chats[&ChatId(7)].clone();
    chat.folder_ids.push(ChatFolderId(3));
    engine.execute(Command::UpsertChat { chat }).unwrap();
    engine
        .execute(Command::SetChatDraft {
            chat_id: ChatId(7),
            draft: Some(ChatDraft {
                content: FormattedText::plain("尚未发送"),
                reply_to_message_id: None,
                updated_at_unix_ms: 500,
            }),
        })
        .unwrap();
    engine
        .execute(Command::SetChatMarkedUnread {
            chat_id: ChatId(7),
            marked_unread: true,
        })
        .unwrap();
    engine
        .execute(Command::SetTypingActions {
            chat_id: ChatId(7),
            actions: vec![TypingAction {
                user_id: UserId(12),
                kind: TypingActionKind::Typing,
                progress_percent: None,
                expires_at_unix_ms: 2_000,
            }],
        })
        .unwrap();

    let remote = Message {
        id: MessageId(101),
        chat_id: ChatId(7),
        sender_user_id: UserId(12),
        date_unix_ms: 1_000,
        edit_date_unix_ms: None,
        content: MessageContent::Text(FormattedText::plain("远端消息")),
        reply_to_message_id: None,
        message_thread_id: None,
        delivery_state: DeliveryState::Sent,
        reactions: Vec::new(),
        is_outgoing: false,
        is_pinned: false,
        is_deleted: false,
    };
    engine
        .execute(Command::UpsertRemoteMessage { message: remote })
        .unwrap();
    engine
        .execute(Command::SetMessageReaction {
            chat_id: ChatId(7),
            message_id: MessageId(101),
            reaction: ReactionCount {
                reaction: "🙏".to_string(),
                total_count: 2,
                chosen_by_me: true,
            },
        })
        .unwrap();

    let chat = &engine.state().chats[&ChatId(7)];
    assert!(chat.is_marked_unread);
    assert_eq!(chat.draft.as_ref().unwrap().content.text, "尚未发送");
    assert_eq!(engine.state().typing_actions[&ChatId(7)].len(), 1);
    assert_eq!(
        engine.state().messages[&(ChatId(7), MessageId(101))].reactions[0].total_count,
        2
    );

    engine
        .execute(Command::DeleteChatFolder {
            folder_id: ChatFolderId(3),
        })
        .unwrap();
    assert!(engine.state().chat_folders.is_empty());
    assert!(engine.state().chats[&ChatId(7)].folder_ids.is_empty());
}
