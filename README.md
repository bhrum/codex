# 大乘 CLI（Mahayana CLI）

本分支是基于 Codex Rust 源码维护的大乘发行版。`mahayana-rs` 产品层在编译期直接链接
本仓库的 `codex-rs` crate，最终只发布一个 `mahayana` 可执行文件和对应的
`libmahayana_runtime` SDK；运行时不会查找、启动或调用另一个官方 `codex` CLI，也不使用
远程 Agent gateway。Flutter、移动端和 WebAssembly 外壳共享同一套大乘 JSON 命令与会话
模型，官方账号、支付宝账号及 DeepSeek provider 均由大乘层统一配置。

保留 `codex-rs/` 与 `mahayana-rs/` 两个源码目录，是为了能够审计并同步 OpenAI 上游变更，
不是两个运行时。桌面构建把二者静态组合进同一个进程；iOS/Android 把同一 Codex Agent
Core 编译为静态库/动态库并通过同一 Rust SDK ABI 在 App 进程内运行，不会退回轻量云端
Agent。由于 `rusty_v8` 没有移动端产物，手机构建只关闭桌面 Deno/V8 code-mode 工具，仍
保留会话、模型、审批、联系人以及 App 私有工作区文件工具。Web 采用更适合浏览器的懒加载
Worker + Rust/WASM + OPFS 路径，模型请求从浏览器 Runtime 直接发出，不经过云端 Agent。

## 从当前改造源码安装

在仓库根目录执行：

```shell
cargo install \
  --locked \
  --force \
  --path mahayana-rs/mahayana-cli
```

安装得到的命令是 `mahayana`，不是 `codex`：

```shell
mahayana status
mahayana contacts
mahayana chat bot-father
mahayana chat codex
```

开发测试也直接面向当前源码：

```shell
cargo test --locked \
  --manifest-path mahayana-rs/Cargo.toml \
  --package mahayana-agent-codex \
  --package mahayana-agent-responses \
  --package mahayana-product \
  --package mahayana-cli
```

桌面 App 构建脚本会从同一个工作区生成 `libmahayana_runtime` 并嵌入应用；发布脚本会同时
打包 `mahayana` 与 SDK，不要求用户另外安装官方 Codex。上游来源、审计基线和同步流程见
[`mahayana-rs/UPSTREAM.md`](mahayana-rs/UPSTREAM.md)。

移动端构建入口为 `fabushi/android/build_telegram_runtime.sh` 与
`fabushi/ios/build_telegram_runtime.sh`（历史文件名保留，实际产物是统一的
`libmahayana_runtime`）。Android App Bundle 会按 ABI 拆分动态库，iOS 则由 Xcode 链接
静态库；release profile 已启用符号裁剪、ThinLTO 和单 codegen unit 以控制包体积。

本仓库继续遵守原项目的 Apache-2.0 许可证，详见 [LICENSE](LICENSE)。
