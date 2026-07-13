use crate::domain::{
    Chat, ChatDraft, ChatFolder, ChatFolderId, ChatId, ClientRequestId, DeliveryState, Message,
    MessageContent, MessageId, ReactionCount, TypingAction, UserId,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum Command {
    UpsertChat {
        chat: Chat,
    },
    UpsertChatFolder {
        folder: ChatFolder,
    },
    DeleteChatFolder {
        folder_id: ChatFolderId,
    },
    SetChatDraft {
        chat_id: ChatId,
        draft: Option<ChatDraft>,
    },
    SetChatMarkedUnread {
        chat_id: ChatId,
        marked_unread: bool,
    },
    SetTypingActions {
        chat_id: ChatId,
        actions: Vec<TypingAction>,
    },
    UpsertRemoteMessage {
        message: Message,
    },
    SetMessageReaction {
        chat_id: ChatId,
        message_id: MessageId,
        reaction: ReactionCount,
    },
    QueueMessage {
        chat_id: ChatId,
        local_message_id: MessageId,
        sender_user_id: UserId,
        client_request_id: ClientRequestId,
        date_unix_ms: i64,
        content: MessageContent,
        reply_to_message_id: Option<MessageId>,
        message_thread_id: Option<MessageId>,
    },
    AcknowledgeMessage {
        client_request_id: ClientRequestId,
        server_message_id: MessageId,
        date_unix_ms: i64,
    },
    FailMessage {
        client_request_id: ClientRequestId,
        code: String,
        retryable: bool,
    },
    EditMessage {
        chat_id: ChatId,
        message_id: MessageId,
        content: MessageContent,
        edit_date_unix_ms: i64,
    },
    DeleteMessages {
        chat_id: ChatId,
        message_ids: Vec<MessageId>,
    },
    SetChatRead {
        chat_id: ChatId,
        last_read_inbox_message_id: MessageId,
        unread_count: u32,
    },
    SetPinnedMessage {
        chat_id: ChatId,
        message_id: Option<MessageId>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum Event {
    ChatUpserted {
        chat: Chat,
    },
    ChatFolderUpserted {
        folder: ChatFolder,
    },
    ChatFolderDeleted {
        folder_id: ChatFolderId,
    },
    ChatDraftUpdated {
        chat_id: ChatId,
        draft: Option<ChatDraft>,
    },
    ChatMarkedUnreadUpdated {
        chat_id: ChatId,
        marked_unread: bool,
    },
    TypingActionsUpdated {
        chat_id: ChatId,
        actions: Vec<TypingAction>,
    },
    RemoteMessageUpserted {
        message: Message,
    },
    MessageReactionUpdated {
        chat_id: ChatId,
        message_id: MessageId,
        reaction: ReactionCount,
    },
    MessageQueued {
        message: Message,
    },
    MessageAcknowledged {
        chat_id: ChatId,
        local_message_id: MessageId,
        server_message_id: MessageId,
        date_unix_ms: i64,
    },
    MessageFailed {
        chat_id: ChatId,
        message_id: MessageId,
        code: String,
        retryable: bool,
    },
    MessageEdited {
        chat_id: ChatId,
        message_id: MessageId,
        content: MessageContent,
        edit_date_unix_ms: i64,
    },
    MessagesDeleted {
        chat_id: ChatId,
        message_ids: Vec<MessageId>,
    },
    ChatReadUpdated {
        chat_id: ChatId,
        last_read_inbox_message_id: MessageId,
        unread_count: u32,
    },
    PinnedMessageUpdated {
        chat_id: ChatId,
        message_id: Option<MessageId>,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramState {
    pub chats: BTreeMap<ChatId, Chat>,
    #[serde(default)]
    pub chat_folders: BTreeMap<ChatFolderId, ChatFolder>,
    #[serde(with = "message_map_serde")]
    pub messages: BTreeMap<(ChatId, MessageId), Message>,
    pub pending_requests: BTreeMap<ClientRequestId, (ChatId, MessageId)>,
    #[serde(default)]
    pub typing_actions: BTreeMap<ChatId, Vec<TypingAction>>,
}

mod message_map_serde {
    use super::*;

    pub fn serialize<S>(
        messages: &BTreeMap<(ChatId, MessageId), Message>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        messages.values().collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<(ChatId, MessageId), Message>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let messages = Vec::<Message>::deserialize(deserializer)?;
        Ok(messages
            .into_iter()
            .map(|message| ((message.chat_id, message.id), message))
            .collect())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EngineError {
    #[error("chat {0:?} does not exist")]
    ChatNotFound(ChatId),
    #[error("message {message_id:?} does not exist in chat {chat_id:?}")]
    MessageNotFound {
        chat_id: ChatId,
        message_id: MessageId,
    },
    #[error("message {message_id:?} already exists in chat {chat_id:?}")]
    DuplicateMessage {
        chat_id: ChatId,
        message_id: MessageId,
    },
    #[error("client request id is empty or too long")]
    InvalidClientRequestId,
    #[error("client request id {0:?} already exists")]
    DuplicateClientRequest(ClientRequestId),
    #[error("pending client request {0:?} was not found")]
    PendingRequestNotFound(ClientRequestId),
    #[error("server message id {message_id:?} already exists in chat {chat_id:?}")]
    DuplicateServerMessage {
        chat_id: ChatId,
        message_id: MessageId,
    },
    #[error("message id list must not be empty")]
    EmptyMessageList,
    #[error("chat folder {0:?} does not exist")]
    ChatFolderNotFound(ChatFolderId),
    #[error("remote message must have sent delivery state")]
    InvalidRemoteMessageState,
    #[error("reaction identifier must not be empty")]
    InvalidReaction,
}

#[derive(Debug, Default, Clone)]
pub struct TelegramEngine {
    state: TelegramState,
}

impl TelegramEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_state(state: TelegramState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &TelegramState {
        &self.state
    }

    pub fn into_state(self) -> TelegramState {
        self.state
    }

    pub fn execute(&mut self, command: Command) -> Result<Vec<Event>, EngineError> {
        let events = self.decide(command)?;
        for event in &events {
            self.apply(event.clone());
        }
        Ok(events)
    }

    pub fn decide(&self, command: Command) -> Result<Vec<Event>, EngineError> {
        match command {
            Command::UpsertChat { chat } => Ok(vec![Event::ChatUpserted { chat }]),
            Command::UpsertChatFolder { folder } => Ok(vec![Event::ChatFolderUpserted { folder }]),
            Command::DeleteChatFolder { folder_id } => {
                if !self.state.chat_folders.contains_key(&folder_id) {
                    return Err(EngineError::ChatFolderNotFound(folder_id));
                }
                Ok(vec![Event::ChatFolderDeleted { folder_id }])
            }
            Command::SetChatDraft { chat_id, draft } => {
                self.require_chat(chat_id)?;
                Ok(vec![Event::ChatDraftUpdated { chat_id, draft }])
            }
            Command::SetChatMarkedUnread {
                chat_id,
                marked_unread,
            } => {
                self.require_chat(chat_id)?;
                Ok(vec![Event::ChatMarkedUnreadUpdated {
                    chat_id,
                    marked_unread,
                }])
            }
            Command::SetTypingActions { chat_id, actions } => {
                self.require_chat(chat_id)?;
                Ok(vec![Event::TypingActionsUpdated { chat_id, actions }])
            }
            Command::UpsertRemoteMessage { message } => {
                self.require_chat(message.chat_id)?;
                if !matches!(message.delivery_state, DeliveryState::Sent) {
                    return Err(EngineError::InvalidRemoteMessageState);
                }
                Ok(vec![Event::RemoteMessageUpserted { message }])
            }
            Command::SetMessageReaction {
                chat_id,
                message_id,
                reaction,
            } => {
                self.require_message(chat_id, message_id)?;
                if reaction.reaction.trim().is_empty() {
                    return Err(EngineError::InvalidReaction);
                }
                Ok(vec![Event::MessageReactionUpdated {
                    chat_id,
                    message_id,
                    reaction,
                }])
            }
            Command::QueueMessage {
                chat_id,
                local_message_id,
                sender_user_id,
                client_request_id,
                date_unix_ms,
                content,
                reply_to_message_id,
                message_thread_id,
            } => {
                self.require_chat(chat_id)?;
                if self
                    .state
                    .messages
                    .contains_key(&(chat_id, local_message_id))
                {
                    return Err(EngineError::DuplicateMessage {
                        chat_id,
                        message_id: local_message_id,
                    });
                }
                if !client_request_id.is_valid() {
                    return Err(EngineError::InvalidClientRequestId);
                }
                if self.state.pending_requests.contains_key(&client_request_id) {
                    return Err(EngineError::DuplicateClientRequest(client_request_id));
                }

                Ok(vec![Event::MessageQueued {
                    message: Message {
                        id: local_message_id,
                        chat_id,
                        sender_user_id,
                        date_unix_ms,
                        edit_date_unix_ms: None,
                        content,
                        reply_to_message_id,
                        message_thread_id,
                        delivery_state: DeliveryState::Pending { client_request_id },
                        reactions: Vec::new(),
                        is_outgoing: true,
                        is_pinned: false,
                        is_deleted: false,
                    },
                }])
            }
            Command::AcknowledgeMessage {
                client_request_id,
                server_message_id,
                date_unix_ms,
            } => {
                let (chat_id, local_message_id) = self
                    .state
                    .pending_requests
                    .get(&client_request_id)
                    .copied()
                    .ok_or(EngineError::PendingRequestNotFound(client_request_id))?;
                if server_message_id != local_message_id
                    && self
                        .state
                        .messages
                        .contains_key(&(chat_id, server_message_id))
                {
                    return Err(EngineError::DuplicateServerMessage {
                        chat_id,
                        message_id: server_message_id,
                    });
                }
                Ok(vec![Event::MessageAcknowledged {
                    chat_id,
                    local_message_id,
                    server_message_id,
                    date_unix_ms,
                }])
            }
            Command::FailMessage {
                client_request_id,
                code,
                retryable,
            } => {
                let (chat_id, message_id) = self
                    .state
                    .pending_requests
                    .get(&client_request_id)
                    .copied()
                    .ok_or(EngineError::PendingRequestNotFound(client_request_id))?;
                Ok(vec![Event::MessageFailed {
                    chat_id,
                    message_id,
                    code,
                    retryable,
                }])
            }
            Command::EditMessage {
                chat_id,
                message_id,
                content,
                edit_date_unix_ms,
            } => {
                self.require_message(chat_id, message_id)?;
                Ok(vec![Event::MessageEdited {
                    chat_id,
                    message_id,
                    content,
                    edit_date_unix_ms,
                }])
            }
            Command::DeleteMessages {
                chat_id,
                message_ids,
            } => {
                self.require_chat(chat_id)?;
                let unique: BTreeSet<_> = message_ids.into_iter().collect();
                if unique.is_empty() {
                    return Err(EngineError::EmptyMessageList);
                }
                for message_id in &unique {
                    self.require_message(chat_id, *message_id)?;
                }
                Ok(vec![Event::MessagesDeleted {
                    chat_id,
                    message_ids: unique.into_iter().collect(),
                }])
            }
            Command::SetChatRead {
                chat_id,
                last_read_inbox_message_id,
                unread_count,
            } => {
                let chat = self.require_chat(chat_id)?;
                if chat
                    .last_read_inbox_message_id
                    .is_some_and(|current| current >= last_read_inbox_message_id)
                    && chat.unread_count == unread_count
                {
                    return Ok(Vec::new());
                }
                Ok(vec![Event::ChatReadUpdated {
                    chat_id,
                    last_read_inbox_message_id,
                    unread_count,
                }])
            }
            Command::SetPinnedMessage {
                chat_id,
                message_id,
            } => {
                let chat = self.require_chat(chat_id)?;
                if let Some(message_id) = message_id {
                    self.require_message(chat_id, message_id)?;
                }
                if chat.pinned_message_id == message_id {
                    return Ok(Vec::new());
                }
                Ok(vec![Event::PinnedMessageUpdated {
                    chat_id,
                    message_id,
                }])
            }
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::ChatUpserted { chat } => {
                self.state.chats.insert(chat.id, chat);
            }
            Event::ChatFolderUpserted { folder } => {
                self.state.chat_folders.insert(folder.id, folder);
            }
            Event::ChatFolderDeleted { folder_id } => {
                self.state.chat_folders.remove(&folder_id);
                for chat in self.state.chats.values_mut() {
                    chat.folder_ids.retain(|id| *id != folder_id);
                }
            }
            Event::ChatDraftUpdated { chat_id, draft } => {
                if let Some(chat) = self.state.chats.get_mut(&chat_id) {
                    chat.draft = draft;
                }
            }
            Event::ChatMarkedUnreadUpdated {
                chat_id,
                marked_unread,
            } => {
                if let Some(chat) = self.state.chats.get_mut(&chat_id) {
                    chat.is_marked_unread = marked_unread;
                }
            }
            Event::TypingActionsUpdated { chat_id, actions } => {
                if actions.is_empty() {
                    self.state.typing_actions.remove(&chat_id);
                } else {
                    self.state.typing_actions.insert(chat_id, actions);
                }
            }
            Event::RemoteMessageUpserted { message } => {
                if let Some(chat) = self.state.chats.get_mut(&message.chat_id) {
                    chat.last_message_id = Some(message.id);
                }
                self.state
                    .messages
                    .insert((message.chat_id, message.id), message);
            }
            Event::MessageReactionUpdated {
                chat_id,
                message_id,
                reaction,
            } => {
                if let Some(message) = self.state.messages.get_mut(&(chat_id, message_id)) {
                    message
                        .reactions
                        .retain(|existing| existing.reaction != reaction.reaction);
                    if reaction.total_count > 0 {
                        message.reactions.push(reaction);
                    }
                }
            }
            Event::MessageQueued { message } => {
                if let DeliveryState::Pending { client_request_id } = &message.delivery_state {
                    self.state
                        .pending_requests
                        .insert(client_request_id.clone(), (message.chat_id, message.id));
                }
                if let Some(chat) = self.state.chats.get_mut(&message.chat_id) {
                    chat.last_message_id = Some(message.id);
                }
                self.state
                    .messages
                    .insert((message.chat_id, message.id), message);
            }
            Event::MessageAcknowledged {
                chat_id,
                local_message_id,
                server_message_id,
                date_unix_ms,
            } => {
                if let Some(mut message) = self.state.messages.remove(&(chat_id, local_message_id))
                {
                    if let DeliveryState::Pending { client_request_id } = &message.delivery_state {
                        self.state.pending_requests.remove(client_request_id);
                    }
                    message.id = server_message_id;
                    message.date_unix_ms = date_unix_ms;
                    message.delivery_state = DeliveryState::Sent;
                    self.state
                        .messages
                        .insert((chat_id, server_message_id), message);
                    if let Some(chat) = self.state.chats.get_mut(&chat_id) {
                        if chat.last_message_id == Some(local_message_id) {
                            chat.last_message_id = Some(server_message_id);
                        }
                    }
                }
            }
            Event::MessageFailed {
                chat_id,
                message_id,
                code,
                retryable,
            } => {
                if let Some(message) = self.state.messages.get_mut(&(chat_id, message_id)) {
                    if let DeliveryState::Pending { client_request_id } = &message.delivery_state {
                        self.state.pending_requests.remove(client_request_id);
                    }
                    message.delivery_state = DeliveryState::Failed { code, retryable };
                }
            }
            Event::MessageEdited {
                chat_id,
                message_id,
                content,
                edit_date_unix_ms,
            } => {
                if let Some(message) = self.state.messages.get_mut(&(chat_id, message_id)) {
                    message.content = content;
                    message.edit_date_unix_ms = Some(edit_date_unix_ms);
                }
            }
            Event::MessagesDeleted {
                chat_id,
                message_ids,
            } => {
                for message_id in message_ids {
                    if let Some(message) = self.state.messages.get_mut(&(chat_id, message_id)) {
                        message.is_deleted = true;
                        if let DeliveryState::Pending { client_request_id } =
                            &message.delivery_state
                        {
                            self.state.pending_requests.remove(client_request_id);
                        }
                    }
                }
            }
            Event::ChatReadUpdated {
                chat_id,
                last_read_inbox_message_id,
                unread_count,
            } => {
                if let Some(chat) = self.state.chats.get_mut(&chat_id) {
                    if chat
                        .last_read_inbox_message_id
                        .is_none_or(|current| last_read_inbox_message_id >= current)
                    {
                        chat.last_read_inbox_message_id = Some(last_read_inbox_message_id);
                    }
                    chat.unread_count = unread_count;
                }
            }
            Event::PinnedMessageUpdated {
                chat_id,
                message_id,
            } => {
                if let Some(chat) = self.state.chats.get_mut(&chat_id) {
                    if let Some(previous) = chat.pinned_message_id {
                        if let Some(message) = self.state.messages.get_mut(&(chat_id, previous)) {
                            message.is_pinned = false;
                        }
                    }
                    chat.pinned_message_id = message_id;
                    if let Some(message_id) = message_id {
                        if let Some(message) = self.state.messages.get_mut(&(chat_id, message_id)) {
                            message.is_pinned = true;
                        }
                    }
                }
            }
        }
    }

    fn require_chat(&self, chat_id: ChatId) -> Result<&Chat, EngineError> {
        self.state
            .chats
            .get(&chat_id)
            .ok_or(EngineError::ChatNotFound(chat_id))
    }

    fn require_message(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<&Message, EngineError> {
        self.state
            .messages
            .get(&(chat_id, message_id))
            .ok_or(EngineError::MessageNotFound {
                chat_id,
                message_id,
            })
    }
}
