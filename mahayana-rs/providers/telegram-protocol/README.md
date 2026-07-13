# Fabushi Telegram Protocol

本 crate 定义授权状态机、请求关联、MTProto 2.0 帧与加密、auth-key 握手、数据中心路由和 TDLib schema 覆盖审计。它不把任何 GPL 客户端实现复制进 Fabushi。

数据中心目录直接消费 `help.getConfig` 的动态 `dc_options`，不硬编码可能频繁变化的生产 IP。主查询迁移（`PHONE_MIGRATE`、`NETWORK_MIGRATE`、`USER_MIGRATE`）与文件迁移（`FILE_MIGRATE`）保持隔离；媒体请求不会错误地改写账号主 DC。

本地验证固定 schema：

```sh
for schema in td_api.tl telegram_api.tl mtproto_api.tl; do
  curl -fsSL \
    "https://raw.githubusercontent.com/tdlib/td/a17f87c4cff7b90b278d12b91ba0614383aaee82/td/generate/scheme/$schema" \
    -o "/tmp/$schema"
done
cargo run --manifest-path native/telegram-protocol/Cargo.toml --bin td-schema-audit -- td /tmp/td_api.tl
cargo run --manifest-path native/telegram-protocol/Cargo.toml --bin td-schema-audit -- telegram /tmp/telegram_api.tl
cargo run --manifest-path native/telegram-protocol/Cargo.toml --bin td-schema-audit -- mtproto /tmp/mtproto_api.tl
```

三层基线分别是：TDLib 高层 API 2,126/1,001、Telegram 线协议 1,631/796、MTProto 核心 40/8（类型声明/函数声明）。每层都必须同时匹配 digest 和声明数。上游升级时必须显式更新 pin、重新生成 Rust 类型并审查功能差异。

运行 `scripts/update-telegram-schema.sh` 会审计三层 schema，并从 2,458 个显式 wire constructor 声明确定性生成 `src/generated/schema_ids.rs`。CI 会重新生成并拒绝任何未提交或不可复现的差异。
