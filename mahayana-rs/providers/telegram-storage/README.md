# Fabushi Telegram Storage

Rust 原生 SQLite 存储，提供：

- XChaCha20-Poly1305 应用层加密；
- 平台 Keychain/Keystore 注入的 256-bit key，数据库不保存 key；
- 带 revision 的原子快照和乐观并发；
- 加密、按序、可裁剪的事件日志；
- 加密消息表与 HMAC-SHA256 盲索引搜索，支持中文字符/双字词、英文词、会话过滤、编辑和删除；
- SQLite schema v1→v2 迁移门禁和崩溃安全事务。

消息 JSON 继续使用 XChaCha20-Poly1305 加密；搜索词只以带主密钥的 HMAC digest 写入数据库，不保存消息正文或可直接反查的明文 token。

当前 crate 面向 Android、iOS、macOS、Windows 和 Linux。Web 端需要保持同一快照/事件接口，但底层改为 IndexedDB；该适配不应降低密文格式或回放语义。
