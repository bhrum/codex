use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    Android,
    Ios,
    Macos,
    Windows,
    Linux,
    Web,
}

pub const ALL_PLATFORMS: &[Platform] = &[
    Platform::Android,
    Platform::Ios,
    Platform::Macos,
    Platform::Windows,
    Platform::Linux,
    Platform::Web,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureDomain {
    Account,
    Chats,
    Messaging,
    Media,
    Communities,
    Calls,
    Stories,
    Discovery,
    Bots,
    Payments,
    Security,
    Notifications,
    Appearance,
    Accessibility,
    PlatformIntegration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RustLayer {
    DomainCore,
    Protocol,
    Storage,
    Media,
    Realtime,
    Security,
    PlatformAdapter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MigrationStatus {
    Planned,
    ContractDefined,
    CorePartial,
    Implemented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    pub key: &'static str,
    pub domain: FeatureDomain,
    pub layer: RustLayer,
    pub platforms: &'static [Platform],
    pub status: MigrationStatus,
}

macro_rules! feature {
    ($key:literal, $domain:ident, $layer:ident, $status:ident) => {
        Feature {
            key: $key,
            domain: FeatureDomain::$domain,
            layer: RustLayer::$layer,
            platforms: ALL_PLATFORMS,
            status: MigrationStatus::$status,
        }
    };
}

/// Cross-platform migration ledger. A feature may move to `Implemented` only
/// after protocol, persistence, UI adapter, and platform acceptance tests pass.
pub const FEATURE_CATALOG: &[Feature] = &[
    feature!("account.phone_auth", Account, Protocol, CorePartial),
    feature!("account.qr_auth", Account, Protocol, Planned),
    feature!("account.two_step_verification", Account, Security, Planned),
    feature!("account.multi_account", Account, DomainCore, Planned),
    feature!("account.profile", Account, DomainCore, Planned),
    feature!("account.username_collectibles", Account, Protocol, Planned),
    feature!("account.sessions", Account, Security, Planned),
    feature!("account.delete_export", Account, Protocol, Planned),
    feature!("chats.private", Chats, DomainCore, CorePartial),
    feature!("chats.saved_messages", Chats, DomainCore, ContractDefined),
    feature!("chats.secret", Chats, Security, ContractDefined),
    feature!("chats.archive", Chats, DomainCore, CorePartial),
    feature!("chats.pinning", Chats, DomainCore, CorePartial),
    feature!("chats.folders", Chats, DomainCore, CorePartial),
    feature!("chats.drafts", Chats, Storage, CorePartial),
    feature!("chats.typing_actions", Chats, Realtime, CorePartial),
    feature!("chats.mark_read_unread", Chats, DomainCore, CorePartial),
    feature!("chats.auto_delete", Chats, Protocol, Planned),
    feature!(
        "messaging.text_entities",
        Messaging,
        DomainCore,
        CorePartial
    ),
    feature!("messaging.reply_quote", Messaging, DomainCore, CorePartial),
    feature!("messaging.forward", Messaging, Protocol, Planned),
    feature!("messaging.edit", Messaging, DomainCore, CorePartial),
    feature!("messaging.delete", Messaging, DomainCore, CorePartial),
    feature!("messaging.scheduled", Messaging, Protocol, Planned),
    feature!("messaging.silent_protected", Messaging, Protocol, Planned),
    feature!("messaging.albums", Messaging, DomainCore, Planned),
    feature!("messaging.reactions", Messaging, DomainCore, CorePartial),
    feature!(
        "messaging.polls_quizzes",
        Messaging,
        DomainCore,
        ContractDefined
    ),
    feature!("messaging.translation", Messaging, Protocol, Planned),
    feature!(
        "messaging.business_quick_replies",
        Messaging,
        Protocol,
        Planned
    ),
    feature!("media.photos_videos", Media, Media, CorePartial),
    feature!("media.files", Media, Media, CorePartial),
    feature!("media.voice_notes", Media, Media, ContractDefined),
    feature!("media.video_notes", Media, Media, ContractDefined),
    feature!("media.music", Media, Media, ContractDefined),
    feature!("media.stickers_emoji", Media, Media, ContractDefined),
    feature!("media.gifs", Media, Media, ContractDefined),
    feature!("media.streaming", Media, Media, Planned),
    feature!("media.download_manager", Media, Storage, CorePartial),
    feature!("media.gallery_editor", Media, PlatformAdapter, Planned),
    feature!("communities.groups", Communities, Protocol, ContractDefined),
    feature!(
        "communities.supergroups",
        Communities,
        Protocol,
        ContractDefined
    ),
    feature!(
        "communities.channels",
        Communities,
        Protocol,
        ContractDefined
    ),
    feature!(
        "communities.forums_topics",
        Communities,
        DomainCore,
        Planned
    ),
    feature!("communities.admin_rights", Communities, Protocol, Planned),
    feature!("communities.moderation", Communities, Protocol, Planned),
    feature!("communities.invite_links", Communities, Protocol, Planned),
    feature!(
        "communities.member_management",
        Communities,
        Protocol,
        Planned
    ),
    feature!("communities.statistics", Communities, Protocol, Planned),
    feature!("calls.voice", Calls, Realtime, Planned),
    feature!("calls.video", Calls, Realtime, Planned),
    feature!("calls.group", Calls, Realtime, Planned),
    feature!("calls.live_streams", Calls, Realtime, Planned),
    feature!("calls.screen_share", Calls, PlatformAdapter, Planned),
    feature!("stories.publish", Stories, Media, ContractDefined),
    feature!("stories.viewer", Stories, Media, Planned),
    feature!("stories.reactions_replies", Stories, Protocol, Planned),
    feature!("stories.privacy_highlights", Stories, Protocol, Planned),
    feature!("discovery.global_search", Discovery, Storage, Planned),
    feature!("discovery.chat_search", Discovery, Storage, Planned),
    feature!("discovery.contacts", Discovery, Protocol, ContractDefined),
    feature!("discovery.nearby", Discovery, Protocol, Planned),
    feature!(
        "discovery.hashtags_public_posts",
        Discovery,
        Protocol,
        Planned
    ),
    feature!("bots.bot_api_interactions", Bots, Protocol, ContractDefined),
    feature!("bots.inline_mode", Bots, Protocol, Planned),
    feature!("bots.reply_keyboards", Bots, DomainCore, Planned),
    feature!("bots.mini_apps", Bots, PlatformAdapter, ContractDefined),
    feature!("bots.games", Bots, PlatformAdapter, Planned),
    feature!("bots.business_bots", Bots, Protocol, Planned),
    feature!("payments.invoices", Payments, Protocol, ContractDefined),
    feature!("payments.stars", Payments, Protocol, Planned),
    feature!("payments.giveaways", Payments, Protocol, Planned),
    feature!("payments.passport", Payments, Security, Planned),
    feature!("security.local_passcode", Security, Security, Planned),
    feature!(
        "security.biometric_lock",
        Security,
        PlatformAdapter,
        Planned
    ),
    feature!("security.privacy_rules", Security, Protocol, Planned),
    feature!("security.block_report", Security, Protocol, Planned),
    feature!("security.secret_chat_e2e", Security, Security, Planned),
    feature!(
        "security.encrypted_local_storage",
        Security,
        Storage,
        CorePartial
    ),
    feature!(
        "notifications.push",
        Notifications,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "notifications.per_chat",
        Notifications,
        DomainCore,
        CorePartial
    ),
    feature!(
        "notifications.custom_sounds",
        Notifications,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "notifications.badges_actions",
        Notifications,
        PlatformAdapter,
        Planned
    ),
    feature!("appearance.themes", Appearance, PlatformAdapter, Planned),
    feature!(
        "appearance.wallpapers",
        Appearance,
        PlatformAdapter,
        Planned
    ),
    feature!("appearance.chat_settings", Appearance, DomainCore, Planned),
    feature!(
        "appearance.localization",
        Appearance,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "accessibility.screen_reader",
        Accessibility,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "accessibility.dynamic_type",
        Accessibility,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "accessibility.keyboard_navigation",
        Accessibility,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.share_extensions",
        PlatformIntegration,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.deep_links",
        PlatformIntegration,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.widgets",
        PlatformIntegration,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.desktop_tray",
        PlatformIntegration,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.global_shortcuts",
        PlatformIntegration,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.web_pwa",
        PlatformIntegration,
        PlatformAdapter,
        Planned
    ),
    feature!(
        "platform.offline_sync",
        PlatformIntegration,
        Storage,
        Planned
    ),
];
