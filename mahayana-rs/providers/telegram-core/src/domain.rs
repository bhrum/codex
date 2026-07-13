use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident, $inner:ty) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

id_type!(AccountId, i64);
id_type!(UserId, i64);
id_type!(ChatId, i64);
id_type!(MessageId, i64);
id_type!(StoryId, i64);
id_type!(ChatFolderId, i32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientRequestId(pub String);

impl ClientRequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn is_valid(&self) -> bool {
        !self.0.trim().is_empty() && self.0.len() <= 128
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatKind {
    Private,
    BasicGroup,
    Supergroup,
    Channel,
    Secret,
    SavedMessages,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    pub mute_until_unix_ms: Option<i64>,
    pub sound_id: Option<String>,
    pub show_preview: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            mute_until_unix_ms: None,
            sound_id: None,
            show_preview: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: ChatId,
    pub kind: ChatKind,
    pub title: String,
    pub last_message_id: Option<MessageId>,
    pub last_read_inbox_message_id: Option<MessageId>,
    pub last_read_outbox_message_id: Option<MessageId>,
    pub unread_count: u32,
    pub pinned_message_id: Option<MessageId>,
    pub notification_settings: NotificationSettings,
    pub is_archived: bool,
    #[serde(default)]
    pub is_marked_unread: bool,
    #[serde(default)]
    pub draft: Option<ChatDraft>,
    #[serde(default)]
    pub folder_ids: Vec<ChatFolderId>,
}

impl Chat {
    pub fn new(id: ChatId, kind: ChatKind, title: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            title: title.into(),
            last_message_id: None,
            last_read_inbox_message_id: None,
            last_read_outbox_message_id: None,
            unread_count: 0,
            pinned_message_id: None,
            notification_settings: NotificationSettings::default(),
            is_archived: false,
            is_marked_unread: false,
            draft: None,
            folder_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDraft {
    pub content: FormattedText,
    pub reply_to_message_id: Option<MessageId>,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFolder {
    pub id: ChatFolderId,
    pub title: String,
    pub icon_name: Option<String>,
    pub included_chat_ids: Vec<ChatId>,
    pub excluded_chat_ids: Vec<ChatId>,
    pub include_contacts: bool,
    pub include_non_contacts: bool,
    pub include_groups: bool,
    pub include_channels: bool,
    pub include_bots: bool,
    pub exclude_muted: bool,
    pub exclude_read: bool,
    pub exclude_archived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TypingActionKind {
    Typing,
    RecordingVoice,
    RecordingVideoNote,
    UploadingPhoto,
    UploadingVideo,
    UploadingDocument,
    ChoosingSticker,
    ChoosingLocation,
    WatchingAnimation,
    SpeakingInGroupCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypingAction {
    pub user_id: UserId,
    pub kind: TypingActionKind,
    pub progress_percent: Option<u8>,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormattedText {
    pub text: String,
    pub entities: Vec<TextEntity>,
}

impl FormattedText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            entities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEntity {
    pub offset_utf16: u32,
    pub length_utf16: u32,
    pub kind: TextEntityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "value"
)]
pub enum TextEntityKind {
    Mention,
    Hashtag,
    Cashtag,
    BotCommand,
    Url,
    Email,
    PhoneNumber,
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Spoiler,
    Code,
    Pre { language: Option<String> },
    TextUrl(String),
    MentionUser(UserId),
    CustomEmoji(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub local_path: Option<String>,
    pub remote_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollOption {
    pub text: String,
    pub voter_count: u32,
    pub is_chosen: bool,
    pub is_correct: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum MessageContent {
    Text(FormattedText),
    Photo {
        file: FileRef,
        caption: FormattedText,
        width: u32,
        height: u32,
        has_spoiler: bool,
    },
    Video {
        file: FileRef,
        caption: FormattedText,
        duration_seconds: u32,
        width: u32,
        height: u32,
        has_spoiler: bool,
    },
    Animation {
        file: FileRef,
        caption: FormattedText,
        duration_seconds: u32,
    },
    Audio {
        file: FileRef,
        caption: FormattedText,
        duration_seconds: u32,
        title: Option<String>,
        performer: Option<String>,
    },
    VoiceNote {
        file: FileRef,
        duration_seconds: u32,
        waveform: Vec<u8>,
        caption: FormattedText,
    },
    VideoNote {
        file: FileRef,
        duration_seconds: u32,
        length: u32,
    },
    Document {
        file: FileRef,
        caption: FormattedText,
    },
    Sticker {
        file: FileRef,
        emoji: String,
        set_id: Option<String>,
        is_animated: bool,
        is_video: bool,
    },
    Poll {
        question: FormattedText,
        options: Vec<PollOption>,
        is_anonymous: bool,
        allows_multiple_answers: bool,
        quiz_explanation: Option<FormattedText>,
    },
    Contact {
        phone_number: String,
        first_name: String,
        last_name: String,
        user_id: Option<UserId>,
    },
    Location {
        latitude: f64,
        longitude: f64,
        live_period_seconds: Option<u32>,
    },
    Venue {
        latitude: f64,
        longitude: f64,
        title: String,
        address: String,
        provider: Option<String>,
        venue_id: Option<String>,
    },
    Dice {
        emoji: String,
        value: u8,
    },
    Story {
        story_id: StoryId,
        via_mention: bool,
    },
    Invoice {
        title: String,
        description: String,
        currency: String,
        total_amount_minor: i64,
    },
    Service {
        action: String,
    },
    Unsupported {
        constructor: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
pub enum DeliveryState {
    Pending { client_request_id: ClientRequestId },
    Sent,
    Failed { code: String, retryable: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionCount {
    pub reaction: String,
    pub total_count: u32,
    pub chosen_by_me: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: MessageId,
    pub chat_id: ChatId,
    pub sender_user_id: UserId,
    pub date_unix_ms: i64,
    pub edit_date_unix_ms: Option<i64>,
    pub content: MessageContent,
    pub reply_to_message_id: Option<MessageId>,
    pub message_thread_id: Option<MessageId>,
    pub delivery_state: DeliveryState,
    pub reactions: Vec<ReactionCount>,
    pub is_outgoing: bool,
    pub is_pinned: bool,
    pub is_deleted: bool,
}
