use super::*;
use crate::ConversationId;
use crate::PeerKind;
use pretty_assertions::assert_eq;

fn conversation(id: &str, title: &str, peer: PeerKind) -> Conversation {
    Conversation {
        id: ConversationId(id.to_string()),
        title: title.to_string(),
        peer,
        pinned: false,
        unread_count: 0,
        updated_at_ms: 0,
    }
}

#[test]
fn registry_uses_stable_mentions_and_resolves_mentions_or_ids() {
    let registry = CapabilityRegistry::from_conversations(
        [
            conversation("codex:agent:assistant", "大乘 AI", PeerKind::CodexAi),
            conversation(
                "miniapp:bot-father",
                "Bot Father",
                PeerKind::MiniApp {
                    app_id: "bot-father".to_string(),
                },
            ),
        ],
        BuildProfile::DesktopFull,
    );

    let capabilities = registry.list(None);
    assert_eq!(
        capabilities
            .iter()
            .map(|capability| (
                capability.id.as_str(),
                capability.mention.as_str(),
                capability.kind,
            ))
            .collect::<Vec<_>>(),
        vec![
            ("agent.mahayana", "@agent.mahayana", CapabilityKind::Agent),
            (
                "miniapp.bot-father",
                "@miniapp.bot-father",
                CapabilityKind::Bot
            ),
        ]
    );
    assert_eq!(
        registry.resolve("@miniapp.bot-father"),
        registry.resolve("miniapp.bot-father")
    );
}

#[test]
fn registry_filters_by_title_id_mention_and_description() {
    let registry = CapabilityRegistry::from_conversations(
        [conversation(
            "miniapp:faliu-flashcards",
            "法流记忆卡",
            PeerKind::MiniApp {
                app_id: "faliu-flashcards".to_string(),
            },
        )],
        BuildProfile::DesktopFull,
    );

    assert_eq!(registry.list(Some("法流")).len(), 1);
    assert_eq!(registry.list(Some("FLASHCARDS")).len(), 1);
    assert_eq!(registry.list(Some("共享插件")).len(), 1);
    assert!(registry.list(Some("missing")).is_empty());
}

#[test]
fn auto_confirm_exposes_permission_and_platform_availability() {
    let conversation = conversation(
        "miniapp:chatgpt-auto-confirm",
        "ChatGPT 自动确认",
        PeerKind::MiniApp {
            app_id: CHATGPT_AUTO_CONFIRM_PLUGIN_ID.to_string(),
        },
    );

    let desktop = capability_from_conversation(&conversation, BuildProfile::DesktopFull);
    assert_eq!(desktop.id, CHATGPT_AUTO_CONFIRM_CAPABILITY_ID);
    assert_eq!(desktop.kind, CapabilityKind::Bot);
    assert_eq!(
        desktop.availability,
        CapabilityAvailability::PermissionRequired
    );
    assert!(desktop.is_invokable());

    let web = capability_from_conversation(&conversation, BuildProfile::WebWasm);
    assert_eq!(web.availability, CapabilityAvailability::Unavailable);
    assert!(!web.is_invokable());
    assert_eq!(
        web.unavailable_reason.as_deref(),
        Some("该能力需要大乘桌面端辅助功能权限")
    );
}

#[test]
fn registry_deduplicates_capabilities_by_stable_id() {
    let conversation = conversation(
        "miniapp:bot-father",
        "Bot Father",
        PeerKind::MiniApp {
            app_id: "bot-father".to_string(),
        },
    );
    let registry = CapabilityRegistry::from_conversations(
        [conversation.clone(), conversation],
        BuildProfile::DesktopFull,
    );

    assert_eq!(registry.list(None).len(), 1);
}
