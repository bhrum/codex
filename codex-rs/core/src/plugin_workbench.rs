//! Codex orchestration profiles for MCP Apps that intentionally build plugins.
//!
//! Ordinary MCP Apps stay tool-isolated and read-only. A marketplace may opt a
//! dedicated builder into this profile by using the stable Bot Father plugin
//! identity. The host still supplies the workspace roots, sandbox, and approval
//! policy; this module never grants access outside those roots.

pub const BOT_FATHER_PLUGIN_ID: &str = "bot-father";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAppOrchestrationProfile {
    pub workspace_builder: bool,
    pub base_instructions: String,
    pub developer_instructions: String,
}

pub fn mcp_app_orchestration_profile(plugin_id: &str, server: &str) -> McpAppOrchestrationProfile {
    if plugin_id == BOT_FATHER_PLUGIN_ID {
        return McpAppOrchestrationProfile {
            workspace_builder: true,
            base_instructions: format!(
                "你是大乘 CLI 的机器人之父，由 Codex 负责插件小程序工程编排。MCP Server `{server}` 提供产品级插件能力；Codex 的工作区工具负责检查、创建、修复和测试真实文件。\n\n\
                 每次处理插件都必须：读取适用的 AGENTS.md；确认目标和现有改动；把插件放在 .agents/plugins/plugins/<name> 并维护 marketplace.json；生成 MCP Tools、MCP App UI、测试、内容和部署文件；为 CLI/desktop 提供可独立启动的本地运行时，为 mobile/web 提供 WASM 或 HTTPS 运行时；使用大乘 CLI validate/test/pack/publish 做闭环；遇到实际运行问题时先写回归测试再修模板或 Codex 编排源；未经验证不得声称已发布或可安装。\n\n\
                 只修改用户指定仓库和宿主给出的 workspace roots。保留无关改动，不读取或输出凭据，不绕过系统权限，不把敏感或破坏性操作加入自动批准。"
            ),
            developer_instructions: "把机器人之父当作通用插件工程 Agent，而不是返回示例 JSON 的脚手架。优先实际执行、验证并修复；MCP Tool 结果是输入和状态，不替代源码检查、测试与发布回执。所有文件写入和命令执行继续服从 Codex sandbox 与宿主审批。".into(),
        };
    }

    McpAppOrchestrationProfile {
        workspace_builder: false,
        base_instructions: format!(
            "你在插件 `{plugin_id}` 的隔离 MCP 小程序会话中。只能使用 Server `{server}` 提供的 MCP Tools；不得使用 shell、文件系统、其他插件或其他 MCP Server。"
        ),
        developer_instructions:
            "普通文本应通过当前 MCP Tools 完成；所有副作用都服从 MCP annotations 和宿主审批。"
                .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_father_gets_the_bounded_workspace_builder_profile() {
        let profile = mcp_app_orchestration_profile(BOT_FATHER_PLUGIN_ID, "bot-father-local");
        assert!(profile.workspace_builder);
        assert!(profile.base_instructions.contains("AGENTS.md"));
        assert!(
            profile
                .base_instructions
                .contains("validate/test/pack/publish")
        );
        assert!(profile.base_instructions.contains("可独立启动的本地运行时"));
    }

    #[test]
    fn ordinary_apps_remain_tool_isolated() {
        let profile = mcp_app_orchestration_profile("weather", "weather-http");
        assert!(!profile.workspace_builder);
        assert!(profile.base_instructions.contains("不得使用 shell"));
    }
}
