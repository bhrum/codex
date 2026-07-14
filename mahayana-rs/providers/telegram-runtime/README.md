# Fabushi Telegram Runtime

把 `telegram-core` 与 `telegram-protocol` 通过稳定 C/JSON ABI 暴露给 Flutter：

- `fabushi_telegram_create_client`
- `fabushi_telegram_create_persistent_client`
- `fabushi_telegram_execute`
- `fabushi_telegram_close_client`
- `fabushi_telegram_free_string`
- `fabushi_telegram_force_link`（iOS 静态库防裁剪锚点）

持久化 client 使用平台注入的 32-byte key 恢复 XChaCha20-Poly1305 加密快照；每次 core command 的事件与新快照在同一 SQLite 事务中提交。临时 client 仍明确报告 `persistentStorage: false`。`telegram.bootstrapTransport` 会通过 Rust TCP/MTProto 握手建立内存中的认证会话，并在返回就绪前完成一次加密 ping/pong 验证；状态只暴露密钥编号，不返回 256-byte 密钥材料，关闭 client 时密钥会自动清零。

下一步为 Android/iOS/macOS/Windows/Linux 产出动态库、为 Web 产出 WASM 包，并把平台 Keychain/Keystore 的 key 生命周期接到 Flutter 服务。
