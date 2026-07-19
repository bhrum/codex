use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const HOME_SCHEMA: &str = "mahayana.miniapp.home.v1";
pub const MAX_HOME_BYTES: usize = 32 * 1024;
pub const MAX_HOME_FEED_ITEMS: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeDocument {
    pub schema: String,
    pub revision: String,
    pub app: AppSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<Welcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tips: Vec<Tip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quick_replies: Vec<QuickReply>,
    #[serde(default)]
    pub feed: Feed,
}

impl HomeDocument {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema != HOME_SCHEMA {
            return Err(ContractError::Schema(self.schema.clone()));
        }
        if self.revision.trim().is_empty() || self.app.id.trim().is_empty() {
            return Err(ContractError::MissingStableId);
        }
        if self.feed.items.len() > MAX_HOME_FEED_ITEMS {
            return Err(ContractError::TooManyFeedItems(self.feed.items.len()));
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ContractError::Invalid(error.to_string()))?
            .len();
        if bytes > MAX_HOME_BYTES {
            return Err(ContractError::TooLarge(bytes));
        }
        Ok(())
    }

    pub fn from_tool_result(result: &Value) -> Result<Option<Self>, ContractError> {
        let Some(content) = extract_structured_content(result) else {
            return Ok(None);
        };
        let Some(schema) = content.get("schema").and_then(Value::as_str) else {
            return Ok(None);
        };
        if schema != HOME_SCHEMA {
            return Ok(None);
        }
        let home: Self = serde_json::from_value(content.clone())
            .map_err(|error| ContractError::Invalid(error.to_string()))?;
        home.validate()?;
        Ok(Some(home))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSummary {
    pub id: String,
    pub title: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    pub id: String,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tip {
    pub id: String,
    pub markdown: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickReply {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub action: QuickAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum QuickAction {
    Message {
        value: String,
    },
    Tool {
        name: String,
        #[serde(default)]
        arguments: Value,
    },
    Resource {
        uri: String,
    },
    Url {
        url: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    #[serde(default)]
    pub items: Vec<FeedItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    pub id: String,
    pub revision: String,
    pub kind: FeedItemKind,
    pub title: String,
    pub published_at: String,
    pub resource_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quick_replies: Vec<QuickReply>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeedItemKind {
    Announcement,
    Article,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDisposition {
    pub handled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_request: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl ChatDisposition {
    pub fn from_tool_result(result: &Value) -> Result<Self, ContractError> {
        let content = extract_structured_content(result)
            .ok_or_else(|| ContractError::Invalid("chat result has no structuredContent".into()))?;
        serde_json::from_value(content.clone())
            .map_err(|error| ContractError::Invalid(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RichMessage {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article: Option<FeedItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_request: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentReceipt {
    pub item_id: String,
    pub revision: String,
    pub read_at_ms: i64,
}

pub fn extract_structured_content(result: &Value) -> Option<&Value> {
    result
        .get("structuredContent")
        .or_else(|| result.get("structured_content"))
}

pub fn mcp_result_text(result: &Value) -> String {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        return text;
    }
    serde_json::to_string_pretty(extract_structured_content(result).unwrap_or(result))
        .unwrap_or_else(|_| "MCP Tool 已完成".into())
}

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("unsupported mini-app home schema: {0}")]
    Schema(String),
    #[error("mini-app content is missing a stable id or revision")]
    MissingStableId,
    #[error("mini-app home feed has {0} items; at most {MAX_HOME_FEED_ITEMS} are allowed")]
    TooManyFeedItems(usize),
    #[error("mini-app home payload is {0} bytes; at most {MAX_HOME_BYTES} are allowed")]
    TooLarge(usize),
    #[error("invalid mini-app contract: {0}")]
    Invalid(String),
}
