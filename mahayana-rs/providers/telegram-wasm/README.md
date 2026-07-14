# Fabushi Telegram WASM

Web 端复用与原生端相同的 Rust 会话、消息、命令/事件和授权状态机，并导出：

- `new TelegramWasmClient()`
- `execute(json)`
- `exportState()`
- `importState(json)`

当前状态会明确报告 `persistentStorage: false` 和 `transportConnected: false`。`exportState` 只向宿主交付快照；在 IndexedDB + WebCrypto/XChaCha 适配完成前，不能把它当作已加密持久化。
