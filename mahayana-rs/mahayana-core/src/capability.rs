use crate::BuildProfile;
use crate::Conversation;
use crate::ConversationId;
use crate::PeerKind;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

pub const MAHAYANA_AGENT_CAPABILITY_ID: &str = "agent.mahayana";
pub const CHATGPT_AUTO_CONFIRM_PLUGIN_ID: &str = "chatgpt-auto-confirm";
pub const CHATGPT_AUTO_CONFIRM_CAPABILITY_ID: &str = "miniapp.chatgpt-auto-confirm";

const BOT_PLUGIN_IDS: [&str; 3] = [
    "bot-father",
    "mahayana-assistant",
    CHATGPT_AUTO_CONFIRM_PLUGIN_ID,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityKind {
    Agent,
    Bot,
    Plugin,
    MiniApp,
    Application,
    Contact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityAvailability {
    Ready,
    PermissionRequired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub id: String,
    pub title: String,
    pub kind: CapabilityKind,
    pub mention: String,
    pub conversation_id: ConversationId,
    pub provider: String,
    pub plugin_id: Option<String>,
    pub description: String,
    pub required_permissions: Vec<String>,
    pub availability: CapabilityAvailability,
    pub unavailable_reason: Option<String>,
}

impl CapabilityDescriptor {
    pub fn is_invokable(&self) -> bool {
        !matches!(self.availability, CapabilityAvailability::Unavailable)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityRegistry {
    capabilities: BTreeMap<String, CapabilityDescriptor>,
}

impl CapabilityRegistry {
    pub fn from_conversations(
        conversations: impl IntoIterator<Item = Conversation>,
        build_profile: BuildProfile,
    ) -> Self {
        let capabilities = conversations
            .into_iter()
            .map(|conversation| capability_from_conversation(&conversation, build_profile))
            .map(|capability| (capability.id.clone(), capability))
            .collect();
        Self { capabilities }
    }

    pub fn list(&self, query: Option<&str>) -> Vec<CapabilityDescriptor> {
        filter_capabilities(self.capabilities.values(), query)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn resolve(&self, selector: &str) -> Option<&CapabilityDescriptor> {
        let selector = selector.trim();
        let id = selector.strip_prefix('@').unwrap_or(selector);
        self.capabilities.get(id).or_else(|| {
            self.capabilities
                .values()
                .find(|capability| capability.mention == selector)
        })
    }
}

pub fn capability_from_conversation(
    conversation: &Conversation,
    build_profile: BuildProfile,
) -> CapabilityDescriptor {
    let (id, kind, plugin_id, description) = match &conversation.peer {
        PeerKind::CodexAi => (
            MAHAYANA_AGENT_CAPABILITY_ID.to_string(),
            CapabilityKind::Agent,
            None,
            "大乘共享智能代理".to_string(),
        ),
        PeerKind::MiniApp { app_id } => (
            format!("miniapp.{app_id}"),
            if BOT_PLUGIN_IDS.contains(&app_id.as_str()) {
                CapabilityKind::Bot
            } else {
                CapabilityKind::MiniApp
            },
            Some(app_id.clone()),
            "大乘共享插件、小程序、应用或机器人能力".to_string(),
        ),
        PeerKind::TelegramContact { user_id } => (
            format!("contact.telegram.{user_id}"),
            CapabilityKind::Contact,
            None,
            "Telegram 联系人机器人".to_string(),
        ),
        PeerKind::MahayanaContact { contact_id } => (
            format!("contact.mahayana.{contact_id}"),
            CapabilityKind::Contact,
            None,
            "大乘联系人机器人".to_string(),
        ),
    };
    let mut required_permissions = Vec::new();
    let mut availability = CapabilityAvailability::Ready;
    let mut unavailable_reason = None;
    if plugin_id.as_deref() == Some(CHATGPT_AUTO_CONFIRM_PLUGIN_ID) {
        required_permissions = vec![
            "desktop.accessibility".to_string(),
            "desktop.chatgpt.approvals".to_string(),
        ];
        match build_profile {
            BuildProfile::DesktopFull => {
                availability = CapabilityAvailability::PermissionRequired;
            }
            BuildProfile::MobileEmbedded | BuildProfile::WebWasm => {
                availability = CapabilityAvailability::Unavailable;
                unavailable_reason = Some("该能力需要大乘桌面端辅助功能权限".to_string());
            }
        }
    }
    CapabilityDescriptor {
        mention: format!("@{id}"),
        id,
        title: conversation.title.clone(),
        kind,
        conversation_id: conversation.id.clone(),
        provider: conversation.peer.provider_key().to_string(),
        plugin_id,
        description,
        required_permissions,
        availability,
        unavailable_reason,
    }
}

pub fn capabilities_from_conversations(
    conversations: impl IntoIterator<Item = Conversation>,
    build_profile: BuildProfile,
) -> Vec<CapabilityDescriptor> {
    CapabilityRegistry::from_conversations(conversations, build_profile).list(None)
}

pub fn filter_capabilities<'a>(
    capabilities: impl IntoIterator<Item = &'a CapabilityDescriptor>,
    query: Option<&str>,
) -> Vec<&'a CapabilityDescriptor> {
    let query = query.map(str::trim).filter(|query| !query.is_empty());
    capabilities
        .into_iter()
        .filter(|capability| {
            query.is_none_or(|query| {
                let query = query.to_lowercase();
                capability.id.to_lowercase().contains(&query)
                    || capability.title.to_lowercase().contains(&query)
                    || capability.mention.to_lowercase().contains(&query)
                    || capability.description.to_lowercase().contains(&query)
            })
        })
        .collect()
}

pub fn find_capability<'a>(
    capabilities: &'a [CapabilityDescriptor],
    selector: &str,
) -> Option<&'a CapabilityDescriptor> {
    let selector = selector.trim();
    let id = selector.strip_prefix('@').unwrap_or(selector);
    capabilities
        .iter()
        .find(|capability| capability.id == id || capability.mention == selector)
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;
