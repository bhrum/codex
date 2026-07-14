use serde_json::json;
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeError {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl RuntimeError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub(crate) fn to_response(&self) -> Value {
        json!({
            "ok": false,
            "errorCode": self.code,
            "message": self.message,
        })
    }

    pub(crate) fn to_runtime_event(&self) -> Value {
        json!({
            "@type": "error",
            "code": self.code,
            "message": self.message,
        })
    }
}
