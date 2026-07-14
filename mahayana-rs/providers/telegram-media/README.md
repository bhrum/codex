# Fabushi Telegram Media

全平台共用的 Rust 媒体传输状态机：

- 上传/下载统一模型；
- Realtime、用户触发、普通、后台四级优先级与 FIFO；
- 可配置并发槽；
- 严格连续分片、断点续传、暂停/恢复、失败/重试/取消；
- 可选 SHA-256 完整性门禁；
- command/event 可序列化，后续可进入加密事件日志。

网络、文件系统、相册和转码由平台/协议 adapter 实现，不在本 crate 中伪造成功。
