use anyhow::Result;
use futures::stream::BoxStream;

pub trait CodexTransport: Send + Sync {
    fn send_message(&mut self, payload: String) -> Result<()>;
    fn receive_stream(&mut self) -> Result<BoxStream<'static, String>>;
    fn terminate(&mut self) -> Result<()>;
}

pub enum CodexTransportKind {
    Subprocess,
    Embedded,
    WebSocket,
}

impl CodexTransportKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodexTransportKind::Subprocess => "subprocess",
            CodexTransportKind::Embedded => "embedded",
            CodexTransportKind::WebSocket => "websocket",
        }
    }
}