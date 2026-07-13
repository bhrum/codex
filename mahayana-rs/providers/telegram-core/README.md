# Fabushi Telegram Core

这是 Fabushi 全平台消息系统的 Rust 领域核心，不是仅用于展示的 Telegram 风格 UI。

当前已落地：

- 跨平台统一的会话、消息、富文本、媒体、投票、故事和支付消息模型；
- 可序列化的 command/event 合约；
- 发送、本地临时 ID、服务端确认、失败、编辑、删除、已读和置顶状态机；
- 90+ 项跨端功能覆盖账本；
- 固定到具体 commit 的官方上游审计基线。

运行验证：

```sh
cargo test --manifest-path native/telegram-core/Cargo.toml
```

协议、加密存储、媒体管线和平台适配器必须作为后续独立 crate 接入，不能把网络或 Flutter 代码塞入本 crate。
