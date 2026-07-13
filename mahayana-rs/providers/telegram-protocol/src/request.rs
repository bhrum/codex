use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolRequest {
    pub id: RequestId,
    pub method: String,
    pub parameters: Map<String, Value>,
}

impl ProtocolRequest {
    pub fn to_td_json(&self) -> Value {
        let mut object = self.parameters.clone();
        object.insert("@type".to_string(), Value::String(self.method.clone()));
        object.insert("@extra".to_string(), Value::String(self.id.0.to_string()));
        Value::Object(object)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RequestError {
    #[error("protocol method must not be empty")]
    EmptyMethod,
    #[error("request sequence exhausted")]
    SequenceExhausted,
}

#[derive(Debug, Clone)]
pub struct RequestSequencer {
    next: u64,
}

impl Default for RequestSequencer {
    fn default() -> Self {
        Self { next: 1 }
    }
}

impl RequestSequencer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(
        &mut self,
        method: impl Into<String>,
        parameters: Map<String, Value>,
    ) -> Result<ProtocolRequest, RequestError> {
        let method = method.into();
        if method.trim().is_empty() {
            return Err(RequestError::EmptyMethod);
        }
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(RequestError::SequenceExhausted)?;
        Ok(ProtocolRequest {
            id: RequestId(id),
            method,
            parameters,
        })
    }
}
