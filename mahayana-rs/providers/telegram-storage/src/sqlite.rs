use crate::EncryptedPayload;
use crate::StorageCipher;
use crate::StorageError;
use crate::StorageKey;
use fabushi_telegram_core::ChatId;
use fabushi_telegram_core::Event;
use fabushi_telegram_core::FormattedText;
use fabushi_telegram_core::Message;
use fabushi_telegram_core::MessageContent;
use fabushi_telegram_core::MessageId;
use fabushi_telegram_core::TelegramState;
use rusqlite::params;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::TransactionBehavior;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

const SCHEMA_VERSION: i64 = 2;
const SNAPSHOT_ID: i64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSnapshot {
    pub revision: u64,
    pub state: TelegramState,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEvent {
    pub sequence: u64,
    pub event: Event,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedTransition {
    pub snapshot: StateSnapshot,
    pub events: Vec<StoredEvent>,
}

pub struct EncryptedSqliteStore {
    connection: Connection,
    cipher: StorageCipher,
}

impl EncryptedSqliteStore {
    pub fn open(path: impl AsRef<Path>, key: StorageKey) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection, key)
    }

    pub fn open_in_memory(key: StorageKey) -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        Self::from_connection(connection, key)
    }

    fn from_connection(connection: Connection, key: StorageKey) -> Result<Self, StorageError> {
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let mut store = Self {
            connection,
            cipher: StorageCipher::new(key),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn load_snapshot(&self) -> Result<Option<StateSnapshot>, StorageError> {
        let row = self
            .connection
            .query_row(
                "SELECT revision, payload, updated_at_unix_ms FROM state_snapshot WHERE id = ?1",
                [SNAPSHOT_ID],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((revision, payload, updated_at_unix_ms)) = row else {
            return Ok(None);
        };
        let revision = to_u64(revision)?;
        let plaintext = self.cipher.decrypt(
            &EncryptedPayload(payload),
            snapshot_associated_data(revision).as_bytes(),
        )?;
        let state = serde_json::from_slice(&plaintext)?;
        Ok(Some(StateSnapshot {
            revision,
            state,
            updated_at_unix_ms,
        }))
    }

    pub fn save_snapshot(
        &mut self,
        state: &TelegramState,
        expected_revision: u64,
        updated_at_unix_ms: i64,
    ) -> Result<StateSnapshot, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction
            .query_row(
                "SELECT revision FROM state_snapshot WHERE id = ?1",
                [SNAPSHOT_ID],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(to_u64)
            .transpose()?
            .unwrap_or(0);
        if current != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                current,
            });
        }
        let revision = current
            .checked_add(1)
            .ok_or(StorageError::IntegerOutOfRange)?;
        let plaintext = serde_json::to_vec(state)?;
        let payload = self
            .cipher
            .encrypt(&plaintext, snapshot_associated_data(revision).as_bytes())?;
        transaction.execute(
            "INSERT INTO state_snapshot (id, revision, payload, updated_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               revision = excluded.revision,
               payload = excluded.payload,
               updated_at_unix_ms = excluded.updated_at_unix_ms",
            params![
                SNAPSHOT_ID,
                to_i64(revision)?,
                payload.0,
                updated_at_unix_ms
            ],
        )?;
        transaction.commit()?;
        Ok(StateSnapshot {
            revision,
            state: state.clone(),
            updated_at_unix_ms,
        })
    }

    pub fn commit_transition(
        &mut self,
        state: &TelegramState,
        events: &[Event],
        expected_revision: u64,
        created_at_unix_ms: i64,
    ) -> Result<PersistedTransition, StorageError> {
        let cipher = &self.cipher;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction
            .query_row(
                "SELECT revision FROM state_snapshot WHERE id = ?1",
                [SNAPSHOT_ID],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(to_u64)
            .transpose()?
            .unwrap_or(0);
        if current != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                current,
            });
        }

        let mut stored_events = Vec::with_capacity(events.len());
        for event in events {
            transaction.execute(
                "INSERT INTO event_log (payload, created_at_unix_ms) VALUES (X'', ?1)",
                [created_at_unix_ms],
            )?;
            let sequence = to_u64(transaction.last_insert_rowid())?;
            let plaintext = serde_json::to_vec(event)?;
            let payload = cipher.encrypt(&plaintext, event_associated_data(sequence).as_bytes())?;
            transaction.execute(
                "UPDATE event_log SET payload = ?1 WHERE sequence = ?2",
                params![payload.0, to_i64(sequence)?],
            )?;
            stored_events.push(StoredEvent {
                sequence,
                event: event.clone(),
                created_at_unix_ms,
            });
        }

        let revision = current
            .checked_add(1)
            .ok_or(StorageError::IntegerOutOfRange)?;
        let plaintext = serde_json::to_vec(state)?;
        let payload = cipher.encrypt(&plaintext, snapshot_associated_data(revision).as_bytes())?;
        transaction.execute(
            "INSERT INTO state_snapshot (id, revision, payload, updated_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               revision = excluded.revision,
               payload = excluded.payload,
               updated_at_unix_ms = excluded.updated_at_unix_ms",
            params![
                SNAPSHOT_ID,
                to_i64(revision)?,
                payload.0,
                created_at_unix_ms
            ],
        )?;
        transaction.commit()?;
        Ok(PersistedTransition {
            snapshot: StateSnapshot {
                revision,
                state: state.clone(),
                updated_at_unix_ms: created_at_unix_ms,
            },
            events: stored_events,
        })
    }

    pub fn append_event(
        &mut self,
        event: &Event,
        created_at_unix_ms: i64,
    ) -> Result<StoredEvent, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO event_log (payload, created_at_unix_ms) VALUES (X'', ?1)",
            [created_at_unix_ms],
        )?;
        let sequence = to_u64(transaction.last_insert_rowid())?;
        let plaintext = serde_json::to_vec(event)?;
        let payload = self
            .cipher
            .encrypt(&plaintext, event_associated_data(sequence).as_bytes())?;
        transaction.execute(
            "UPDATE event_log SET payload = ?1 WHERE sequence = ?2",
            params![payload.0, to_i64(sequence)?],
        )?;
        transaction.commit()?;
        Ok(StoredEvent {
            sequence,
            event: event.clone(),
            created_at_unix_ms,
        })
    }

    pub fn load_events_after(&self, sequence: u64) -> Result<Vec<StoredEvent>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT sequence, payload, created_at_unix_ms
             FROM event_log WHERE sequence > ?1 ORDER BY sequence ASC",
        )?;
        let rows = statement.query_map([to_i64(sequence)?], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (sequence, payload, created_at_unix_ms) = row?;
            let sequence = to_u64(sequence)?;
            let plaintext = self.cipher.decrypt(
                &EncryptedPayload(payload),
                event_associated_data(sequence).as_bytes(),
            )?;
            let event = serde_json::from_slice(&plaintext)?;
            events.push(StoredEvent {
                sequence,
                event,
                created_at_unix_ms,
            });
        }
        Ok(events)
    }

    pub fn upsert_message_index(&mut self, message: &Message) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if message.is_deleted {
            transaction.execute(
                "DELETE FROM message_index WHERE chat_id = ?1 AND message_id = ?2",
                params![message.chat_id.0, message.id.0],
            )?;
            transaction.commit()?;
            return Ok(());
        }

        let plaintext = serde_json::to_vec(message)?;
        let associated_data = message_associated_data(message.chat_id, message.id);
        let payload = self
            .cipher
            .encrypt(&plaintext, associated_data.as_bytes())?;
        transaction.execute(
            "INSERT INTO message_index (
               chat_id, message_id, date_unix_ms, payload
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chat_id, message_id) DO UPDATE SET
               date_unix_ms = excluded.date_unix_ms,
               payload = excluded.payload",
            params![
                message.chat_id.0,
                message.id.0,
                message.date_unix_ms,
                payload.0
            ],
        )?;
        transaction.execute(
            "DELETE FROM message_search_token WHERE chat_id = ?1 AND message_id = ?2",
            params![message.chat_id.0, message.id.0],
        )?;
        for token in search_tokens(&message_search_text(message), 512) {
            let digest = self
                .cipher
                .blind_index_token(b"fabushi.telegram.message-search.v1", token.as_bytes());
            transaction.execute(
                "INSERT OR IGNORE INTO message_search_token (
                   token, chat_id, message_id
                 ) VALUES (?1, ?2, ?3)",
                params![digest.as_slice(), message.chat_id.0, message.id.0],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_message_index(
        &mut self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<bool, StorageError> {
        Ok(self.connection.execute(
            "DELETE FROM message_index WHERE chat_id = ?1 AND message_id = ?2",
            params![chat_id.0, message_id.0],
        )? > 0)
    }

    pub fn load_indexed_message(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<Option<Message>, StorageError> {
        let payload = self
            .connection
            .query_row(
                "SELECT payload FROM message_index
                 WHERE chat_id = ?1 AND message_id = ?2",
                params![chat_id.0, message_id.0],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        self.decrypt_indexed_message(chat_id, message_id, payload)
            .map(Some)
    }

    pub fn search_messages(
        &self,
        query: &str,
        chat_id: Option<ChatId>,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tokens = search_tokens(query, 64);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let digests: Vec<_> = tokens
            .iter()
            .map(|token| {
                self.cipher
                    .blind_index_token(b"fabushi.telegram.message-search.v1", token.as_bytes())
            })
            .collect();
        let placeholders = std::iter::repeat_n("?", digests.len())
            .collect::<Vec<_>>()
            .join(", ");
        let mut sql = format!(
            "SELECT mi.chat_id, mi.message_id, mi.payload
             FROM message_index mi
             JOIN message_search_token mt
               ON mt.chat_id = mi.chat_id AND mt.message_id = mi.message_id
             WHERE mt.token IN ({placeholders})"
        );
        let mut parameters: Vec<Value> = digests
            .iter()
            .map(|digest| Value::Blob(digest.to_vec()))
            .collect();
        if let Some(chat_id) = chat_id {
            sql.push_str(" AND mi.chat_id = ?");
            parameters.push(Value::Integer(chat_id.0));
        }
        sql.push_str(
            " GROUP BY mi.chat_id, mi.message_id, mi.date_unix_ms, mi.payload
              HAVING COUNT(DISTINCT mt.token) = ?
              ORDER BY mi.date_unix_ms DESC, mi.message_id DESC
              LIMIT ?",
        );
        parameters.push(Value::Integer(to_i64(tokens.len() as u64)?));
        parameters.push(Value::Integer(to_i64(limit.min(100) as u64)?));

        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(parameters), |row| {
            Ok((
                ChatId(row.get::<_, i64>(0)?),
                MessageId(row.get::<_, i64>(1)?),
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (chat_id, message_id, payload) = row?;
            messages.push(self.decrypt_indexed_message(chat_id, message_id, payload)?);
        }
        Ok(messages)
    }

    fn decrypt_indexed_message(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        payload: Vec<u8>,
    ) -> Result<Message, StorageError> {
        let associated_data = message_associated_data(chat_id, message_id);
        let plaintext = self
            .cipher
            .decrypt(&EncryptedPayload(payload), associated_data.as_bytes())?;
        Ok(serde_json::from_slice(&plaintext)?)
    }

    pub fn prune_events_through(&mut self, sequence: u64) -> Result<usize, StorageError> {
        Ok(self.connection.execute(
            "DELETE FROM event_log WHERE sequence <= ?1",
            [to_i64(sequence)?],
        )?)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        let current: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current > SCHEMA_VERSION {
            return Err(StorageError::UnsupportedSchemaVersion(current));
        }
        if current == 0 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "CREATE TABLE state_snapshot (
                   id INTEGER PRIMARY KEY CHECK (id = 1),
                   revision INTEGER NOT NULL CHECK (revision > 0),
                   payload BLOB NOT NULL,
                   updated_at_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE event_log (
                   sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                   payload BLOB NOT NULL,
                   created_at_unix_ms INTEGER NOT NULL
                 );
                 CREATE TABLE message_index (
                   chat_id INTEGER NOT NULL,
                   message_id INTEGER NOT NULL,
                   date_unix_ms INTEGER NOT NULL,
                   payload BLOB NOT NULL,
                   PRIMARY KEY (chat_id, message_id)
                 );
                 CREATE INDEX message_index_chronology
                   ON message_index (chat_id, date_unix_ms DESC, message_id DESC);
                 CREATE TABLE message_search_token (
                   token BLOB NOT NULL,
                   chat_id INTEGER NOT NULL,
                   message_id INTEGER NOT NULL,
                   PRIMARY KEY (token, chat_id, message_id),
                   FOREIGN KEY (chat_id, message_id)
                     REFERENCES message_index(chat_id, message_id) ON DELETE CASCADE
                 ) WITHOUT ROWID;
                 PRAGMA user_version = 2;",
            )?;
            transaction.commit()?;
        } else if current == 1 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "CREATE TABLE message_index (
                   chat_id INTEGER NOT NULL,
                   message_id INTEGER NOT NULL,
                   date_unix_ms INTEGER NOT NULL,
                   payload BLOB NOT NULL,
                   PRIMARY KEY (chat_id, message_id)
                 );
                 CREATE INDEX message_index_chronology
                   ON message_index (chat_id, date_unix_ms DESC, message_id DESC);
                 CREATE TABLE message_search_token (
                   token BLOB NOT NULL,
                   chat_id INTEGER NOT NULL,
                   message_id INTEGER NOT NULL,
                   PRIMARY KEY (token, chat_id, message_id),
                   FOREIGN KEY (chat_id, message_id)
                     REFERENCES message_index(chat_id, message_id) ON DELETE CASCADE
                 ) WITHOUT ROWID;
                 PRAGMA user_version = 2;",
            )?;
            transaction.commit()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
}

fn message_associated_data(chat_id: ChatId, message_id: MessageId) -> String {
    format!(
        "fabushi.telegram.message-index.v1:{}:{}",
        chat_id.0, message_id.0
    )
}

fn message_search_text(message: &Message) -> String {
    let mut parts = Vec::new();
    match &message.content {
        MessageContent::Text(text) => push_formatted_text(&mut parts, text),
        MessageContent::Photo { caption, .. }
        | MessageContent::Video { caption, .. }
        | MessageContent::Animation { caption, .. }
        | MessageContent::VoiceNote { caption, .. }
        | MessageContent::Document { caption, .. } => push_formatted_text(&mut parts, caption),
        MessageContent::Audio {
            caption,
            title,
            performer,
            ..
        } => {
            push_formatted_text(&mut parts, caption);
            push_optional(&mut parts, title);
            push_optional(&mut parts, performer);
        }
        MessageContent::VideoNote { .. }
        | MessageContent::Location { .. }
        | MessageContent::Story { .. } => {}
        MessageContent::Sticker { emoji, .. } | MessageContent::Dice { emoji, .. } => {
            parts.push(emoji.as_str());
        }
        MessageContent::Poll {
            question,
            options,
            quiz_explanation,
            ..
        } => {
            push_formatted_text(&mut parts, question);
            for option in options {
                parts.push(option.text.as_str());
            }
            if let Some(explanation) = quiz_explanation {
                push_formatted_text(&mut parts, explanation);
            }
        }
        MessageContent::Contact {
            phone_number,
            first_name,
            last_name,
            ..
        } => {
            parts.extend([
                phone_number.as_str(),
                first_name.as_str(),
                last_name.as_str(),
            ]);
        }
        MessageContent::Venue { title, address, .. } => {
            parts.extend([title.as_str(), address.as_str()]);
        }
        MessageContent::Invoice {
            title,
            description,
            currency,
            ..
        } => {
            parts.extend([title.as_str(), description.as_str(), currency.as_str()]);
        }
        MessageContent::Service { action } => parts.push(action.as_str()),
        MessageContent::Unsupported { constructor } => parts.push(constructor.as_str()),
    }
    parts.join(" ")
}

fn push_formatted_text<'a>(parts: &mut Vec<&'a str>, text: &'a FormattedText) {
    if !text.text.is_empty() {
        parts.push(text.text.as_str());
    }
}

fn push_optional<'a>(parts: &mut Vec<&'a str>, value: &'a Option<String>) {
    if let Some(value) = value.as_deref() {
        parts.push(value);
    }
}

fn search_tokens(input: &str, maximum: usize) -> Vec<String> {
    let mut tokens = BTreeSet::new();
    let mut word = String::new();
    let mut cjk_run = Vec::new();

    for character in input.chars() {
        if is_cjk(character) {
            flush_word(&mut word, &mut tokens);
            cjk_run.push(character);
        } else if character.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            word.extend(character.to_lowercase());
        } else {
            flush_word(&mut word, &mut tokens);
            flush_cjk(&mut cjk_run, &mut tokens);
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);

    tokens.into_iter().take(maximum).collect()
}

fn flush_word(word: &mut String, tokens: &mut BTreeSet<String>) {
    if !word.is_empty() {
        tokens.insert(word.chars().take(128).collect());
        word.clear();
    }
}

fn flush_cjk(run: &mut Vec<char>, tokens: &mut BTreeSet<String>) {
    for &character in run.iter() {
        tokens.insert(character.to_string());
    }
    for pair in run.windows(2) {
        tokens.insert(pair.iter().collect());
    }
    run.clear();
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
    )
}

fn snapshot_associated_data(revision: u64) -> String {
    format!("fabushi.telegram.snapshot.v1:{revision}")
}

fn event_associated_data(sequence: u64) -> String {
    format!("fabushi.telegram.event.v1:{sequence}")
}

fn to_i64(value: u64) -> Result<i64, StorageError> {
    value
        .try_into()
        .map_err(|_| StorageError::IntegerOutOfRange)
}

fn to_u64(value: i64) -> Result<u64, StorageError> {
    value
        .try_into()
        .map_err(|_| StorageError::IntegerOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_is_applied() {
        let store = EncryptedSqliteStore::open_in_memory(StorageKey::from_slice(&[7; 32]).unwrap())
            .unwrap();
        let version: i64 = store
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }
}
