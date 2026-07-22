use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum PluginTemplate {
    Conversational,
    DesktopApproval,
}

impl fmt::Display for PluginTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Conversational => "conversational",
            Self::DesktopApproval => "desktop-approval",
        })
    }
}

fn pretty_json(value: Value) -> Result<String, String> {
    serde_json::to_string_pretty(&value)
        .map(|source| source + "\n")
        .map_err(|error| error.to_string())
}

pub(super) fn template_files(
    name: &str,
    title: &str,
    template: PluginTemplate,
) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    files.insert(
        ".codex-plugin/plugin.json".into(),
        pretty_json(plugin_manifest(name, title, template))?,
    );
    files.insert(
        ".mcp.json".into(),
        pretty_json(mcp_manifest(name, template))?,
    );
    files.insert(
        ".mahayana/plugin.json".into(),
        pretty_json(mahayana_manifest(name, template))?,
    );
    add_content_files(&mut files, title, template);
    add_runtime_files(&mut files, name, title, template)?;
    Ok(files)
}

fn plugin_manifest(name: &str, title: &str, template: PluginTemplate) -> Value {
    let (short_description, long_description, capabilities) = match template {
        PluginTemplate::Conversational => (
            "对话式 MCP 小程序",
            "由仓库内容、MCP Tools 和 Cloudflare UI 组成的小程序。",
            json!(["Interactive"]),
        ),
        PluginTemplate::DesktopApproval => (
            "按精确规则自动处理桌面授权",
            "通过大乘桌面宿主检测 ChatGPT 应用授权卡，只自动放行显式允许列表匹配且不含敏感动作的请求。",
            json!(["Interactive", "Write"]),
        ),
    };
    let remote_platforms = match template {
        PluginTemplate::Conversational => json!(["cli", "desktop", "mobile", "web"]),
        PluginTemplate::DesktopApproval => json!(["cli", "desktop"]),
    };
    let mut manifest = json!({
        "name":name,
        "version":"0.1.0",
        "description":format!("{title} 对话式 MCP 小程序"),
        "author":{"name":format!("{title} 开发者")},
        "repository":"https://github.com/owner/repository",
        "license":"Apache-2.0",
        "mcpServers":"./.mcp.json",
        "interface":{
            "displayName":title,
            "shortDescription":short_description,
            "longDescription":long_description,
            "developerName":format!("{title} 开发者"),
            "category":"Productivity",
            "capabilities":capabilities
        }
    });
    if template == PluginTemplate::Conversational {
        manifest["runtimeVariants"] = json!([
            {"id":"standalone-local","server":format!("{name}-local"),"platforms":["cli","desktop"],"priority":300},
            {"id":"account-http","server":name,"platforms":remote_platforms,"priority":100}
        ]);
    }
    manifest
}

fn mcp_manifest(name: &str, template: PluginTemplate) -> Value {
    let mut servers = Map::new();
    servers.insert(
        format!("{name}-local"),
        json!({"type":"stdio","command":"node","args":["--experimental-strip-types","./server/index.mjs"]}),
    );
    if template == PluginTemplate::Conversational {
        servers.insert(
            name.into(),
            json!({"type":"http","url":format!("https://{name}-mcp.workers.dev/mcp")}),
        );
    }
    json!({"mcpServers":servers})
}

fn mahayana_manifest(_name: &str, template: PluginTemplate) -> Value {
    let (permissions, commands) = match template {
        PluginTemplate::Conversational => (json!(["mcp.call", "storage.local"]), json!([])),
        PluginTemplate::DesktopApproval => (
            json!([
                "mcp.call",
                "storage.local",
                "desktop.accessibility",
                "desktop.chatgpt.approvals"
            ]),
            json!([
                {"name":"start","tool":"start"},
                {"name":"stop","tool":"stop"},
                {"name":"status","tool":"status"},
                {"name":"scan","tool":"scan_once"},
                {"name":"audit","tool":"audit_log"}
            ]),
        ),
    };
    json!({
        "schemaVersion":1,
        "supportedPlatforms": match template {
            PluginTemplate::Conversational => json!(["cli", "desktop", "mobile", "web"]),
            PluginTemplate::DesktopApproval => json!(["cli", "desktop"]),
        },
        "runtime":{
            "cli":{"executable":"node","args":["--experimental-strip-types","./server/index.mjs"]}
        },
        "miniapp":{
            "entry":"./ui/index.html",
            "bridgeVersion":"1.0",
            "permissions":permissions
        },
        "commands":commands,
        "gates":[]
    })
}

fn add_content_files(files: &mut BTreeMap<String, String>, title: &str, template: PluginTemplate) {
    let welcome = match template {
        PluginTemplate::Conversational => {
            format!("欢迎来到 **{title}**。\n\n请在这里说明小程序用途、使用方法和预期效果。")
        }
        PluginTemplate::DesktopApproval => format!(
            "欢迎使用 **{title}**。\n\n先配置应用、动作与资源三个精确匹配条件，再启动桌面监听。敏感动作永不自动确认，所有匹配结果保留本地审计记录。"
        ),
    };
    files.insert(
        "content/welcome.md".into(),
        format!("---\nid: welcome\nrevision: '1'\n---\n{welcome}\n"),
    );
    files.insert(
        "content/tips/getting-started.md".into(),
        "---\nid: getting-started\nrevision: '1'\n---\n回复 `/` 可查看当前 MCP Tools。\n".into(),
    );
    files.insert(
        "content/announcements/launch.md".into(),
        "---\nid: launch\nrevision: '1'\ntitle: 小程序上线\npublishedAt: 2026-07-19\nsummary: 欢迎使用这个对话式 MCP 小程序。\ntags: [公告]\n---\n# 小程序上线\n\n这里是首条公告。\n".into(),
    );
    files.insert(
        "content/articles/guide.md".into(),
        "---\nid: guide\nrevision: '1'\ntitle: 使用指南\npublishedAt: 2026-07-19\nsummary: 了解小程序的核心功能。\ntags: [指南]\n---\n# 使用指南\n\n在这里编写公众号式文章正文。\n".into(),
    );
}

fn add_runtime_files(
    files: &mut BTreeMap<String, String>,
    name: &str,
    title: &str,
    template: PluginTemplate,
) -> Result<(), String> {
    let ui_copy = match template {
        PluginTemplate::Conversational => {
            "Pages UI 已就绪；对话、欢迎内容和文章由宿主通过 MCP 加载。"
        }
        PluginTemplate::DesktopApproval => {
            "设置精确允许规则后启动监听。插件只在本机读取 ChatGPT 授权卡，并保留敏感动作拦截和审计记录。"
        }
    };
    files.insert(
        "ui/index.html".into(),
        format!(
            "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\"><title>{title}</title><style>body{{font:16px system-ui;margin:0;padding:2rem;max-width:48rem}}article{{padding:1.25rem;border:1px solid #ddd;border-radius:1rem}}</style></head><body><article><h1>{title}</h1><p>{ui_copy}</p></article></body></html>\n"
        ),
    );
    files.insert("worker/src/index.ts".into(), worker_source(name, template));
    files.insert("server/index.mjs".into(), LOCAL_STDIO_SERVER_SOURCE.into());
    files.insert(
        "worker/src/content.generated.ts".into(),
        initial_generated_content(name, title)?,
    );
    files.insert(
        "scripts/compile-content.mjs".into(),
        CONTENT_COMPILER_SCRIPT.into(),
    );
    files.insert(
        "package.json".into(),
        pretty_json(json!({
            "name":format!("@mahayana/{name}"),
            "version":"0.1.0",
            "private":true,
            "type":"module",
            "scripts":{
                "build:content":"node scripts/compile-content.mjs",
                "start":"node --experimental-strip-types server/index.mjs",
                "test":"npm run build:content && node --experimental-strip-types --test test/*.test.mjs",
                "deploy":"npm run build:content && npx wrangler deploy"
            },
            "engines":{"node":">=22.6"},
            "devDependencies":{"wrangler":"^4.0.0"}
        }))?,
    );
    files.insert(
        "wrangler.toml".into(),
        format!(
            "name = \"{name}-mcp\"\nmain = \"worker/src/index.ts\"\ncompatibility_date = \"2026-07-19\"\n\n[assets]\ndirectory = \".mahayana-distribution\"\nrun_worker_first = [\"/mcp\", \"/mcp/*\"]\n"
        ),
    );
    files.insert(
        "test/contract.test.mjs".into(),
        format!(
            "import assert from 'node:assert/strict';\nimport test from 'node:test';\nimport worker from '../worker/src/index.ts';\nimport {{ HOME, RESOURCES }} from '../worker/src/content.generated.ts';\n\ntest('home contract', () => {{\n  assert.equal(HOME.schema, 'mahayana.miniapp.home.v1');\n  assert.equal(HOME.app.id, '{name}');\n  assert.ok(Buffer.byteLength(JSON.stringify(HOME)) <= 32768);\n  assert.ok(HOME.feed.items.length <= 10);\n}});\ntest('article bodies stay lazy', () => assert.ok(Object.keys(RESOURCES).length >= 1));\ntest('JSON-RPC errors use the top-level error member', async () => {{\n  const response = await worker.fetch(new Request('https://example.test/mcp', {{\n    method: 'POST', body: JSON.stringify({{ jsonrpc: '2.0', id: 7, method: 'unknown' }}),\n  }}));\n  assert.deepEqual((await response.json()).error, {{ code: -32601, message: 'Method not found' }});\n}});\n"
        ),
    );
    files.insert(
        "test/standalone-runtime.test.mjs".into(),
        STANDALONE_RUNTIME_TEST.into(),
    );
    if template == PluginTemplate::DesktopApproval {
        files.insert(
            "test/desktop-approval.test.mjs".into(),
            DESKTOP_APPROVAL_TEST.replace("__PLUGIN_NAME__", name),
        );
    }
    files.insert(
        ".github/workflows/deploy-cloudflare.yml".into(),
        cloudflare_workflow(name),
    );
    Ok(())
}

fn initial_generated_content(name: &str, title: &str) -> Result<String, String> {
    let home = json!({
        "schema":"mahayana.miniapp.home.v1",
        "revision":"run-npm-build-content",
        "app":{"id":name,"title":title,"version":"0.1.0"},
        "welcome":{"id":"welcome","markdown":format!("欢迎来到 **{title}**。")},
        "tips":[],
        "quickReplies":[],
        "feed":{"items":[],"nextCursor":null}
    });
    Ok(format!(
        "export const HOME = {} as const;\nexport const RESOURCES: Record<string,string> = {{}};\n",
        serde_json::to_string_pretty(&home).map_err(|error| error.to_string())?
    ))
}

fn cloudflare_workflow(name: &str) -> String {
    format!(
        "name: Deploy Cloudflare\non:\n  push:\n    branches: [main]\n  workflow_dispatch:\njobs:\n  deploy:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 22\n      - run: npm install\n      - run: npm run test\n      - run: npx wrangler pages deploy ui --project-name {name}\n        env:\n          CLOUDFLARE_API_TOKEN: ${{{{ secrets.CLOUDFLARE_API_TOKEN }}}}\n          CLOUDFLARE_ACCOUNT_ID: ${{{{ secrets.CLOUDFLARE_ACCOUNT_ID }}}}\n      - run: npx wrangler deploy\n        env:\n          CLOUDFLARE_API_TOKEN: ${{{{ secrets.CLOUDFLARE_API_TOKEN }}}}\n          CLOUDFLARE_ACCOUNT_ID: ${{{{ secrets.CLOUDFLARE_ACCOUNT_ID }}}}\n"
    )
}

fn worker_source(name: &str, template: PluginTemplate) -> String {
    match template {
        PluginTemplate::Conversational => WORKER_SOURCE,
        PluginTemplate::DesktopApproval => DESKTOP_APPROVAL_WORKER_SOURCE,
    }
    .replace("__PLUGIN_NAME__", name)
}

const WORKER_SOURCE: &str = r#"import { HOME, RESOURCES } from './content.generated.ts';

const reply = (id: unknown, result: unknown) => Response.json({ jsonrpc: '2.0', id, result });
const error = (id: unknown, code: number, message: string) => Response.json({
  jsonrpc: '2.0', id, error: { code, message },
});

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname !== '/mcp') return new Response('Not found', { status: 404 });
    if (request.method === 'DELETE') return new Response(null, { status: 204 });
    if (request.method !== 'POST') return new Response('Method not allowed', { status: 405 });
    const rpc = await request.json() as any;
    if (rpc.method === 'notifications/initialized') return new Response(null, { status: 202 });
    if (rpc.method === 'initialize') return reply(rpc.id, {
      protocolVersion: '2025-06-18', capabilities: { tools: {}, resources: {} },
      serverInfo: { name: '__PLUGIN_NAME__', version: '0.1.0' },
    });
    if (rpc.method === 'tools/list') return reply(rpc.id, { tools: [
      { name: 'home', description: '加载小程序首页', annotations: { readOnlyHint: true }, inputSchema: {
        type: 'object', additionalProperties: false, properties: {
          surface: { type: 'string' }, locale: { type: 'string' }, cursor: { type: 'string' },
          limit: { type: 'integer', minimum: 1, maximum: 10 },
        },
      } },
      { name: 'chat', description: '处理小程序对话', inputSchema: {
        type: 'object', additionalProperties: false, required: ['message'], properties: {
          message: { type: 'string' }, surface: { type: 'string' }, locale: { type: 'string' },
          actionId: { type: ['string', 'null'] },
        },
      } },
    ] });
    if (rpc.method === 'tools/call' && rpc.params?.name === 'home') return reply(rpc.id, {
      content: [{ type: 'text', text: HOME.welcome?.markdown ?? '' }], structuredContent: HOME,
    });
    if (rpc.method === 'tools/call' && rpc.params?.name === 'chat') return reply(rpc.id, {
      content: [], structuredContent: { handled: false },
    });
    if (rpc.method === 'resources/list') return reply(rpc.id, { resources: Object.keys(RESOURCES).map(uri => ({
      uri, name: uri.split('/').at(-1), mimeType: 'text/markdown',
    })) });
    if (rpc.method === 'resources/read') {
      const text = RESOURCES[String(rpc.params?.uri ?? '')];
      if (text === undefined) return error(rpc.id, -32002, 'Resource not found');
      return reply(rpc.id, { contents: [{ uri: rpc.params.uri, mimeType: 'text/markdown', text }] });
    }
    return error(rpc.id ?? null, -32601, 'Method not found');
  },
};
"#;

const DESKTOP_APPROVAL_WORKER_SOURCE: &str = r#"import { HOME, RESOURCES } from './content.generated.ts';

const reply = (id: unknown, result: unknown) => Response.json({ jsonrpc: '2.0', id, result });
const error = (id: unknown, code: number, message: string) => Response.json({
  jsonrpc: '2.0', id, error: { code, message },
});
const annotations = (readOnlyHint = false) => ({
  readOnlyHint, destructiveHint: false, openWorldHint: false,
});
const hostResult = (id: unknown, capability: string, params: unknown, approval: 'required' | 'none') =>
  reply(id, {
    content: [{ type: 'text', text: `已向大乘桌面宿主提交 ${capability}。` }],
    structuredContent: {
      handled: true,
      hostRequest: { transport: 'mcp-host-bridge', capability, params, approval },
    },
  });
const ruleSchema = {
  type: 'object', additionalProperties: false,
  required: ['application', 'action', 'resource'],
  properties: {
    application: { type: 'string', minLength: 1, description: '例如 GitHub' },
    action: { type: 'string', minLength: 1, description: '例如 Enable auto-merge' },
    resource: { type: 'string', minLength: 1, description: '例如 bhrumom/fabushi' },
  },
};
const tools = [
  { name: 'home', description: '加载插件首页', annotations: annotations(true), inputSchema: {
    type: 'object', additionalProperties: false, properties: {
      surface: { type: 'string' }, locale: { type: 'string' }, cursor: { type: 'string' },
      limit: { type: 'integer', minimum: 1, maximum: 10 },
    },
  } },
  { name: 'start', description: '使用精确允许规则启动 ChatGPT 授权卡监听', annotations: annotations(), inputSchema: {
    type: 'object', additionalProperties: false, required: ['rules'], properties: {
      rules: { type: 'array', minItems: 1, maxItems: 20, items: ruleSchema },
      intervalMs: { type: 'integer', minimum: 400, maximum: 5000, default: 750 },
    },
  } },
  { name: 'stop', description: '停止自动确认监听', annotations: annotations(), inputSchema: {
    type: 'object', additionalProperties: false, properties: {},
  } },
  { name: 'status', description: '读取监听、权限和规则状态', annotations: annotations(true), inputSchema: {
    type: 'object', additionalProperties: false, properties: {},
  } },
  { name: 'scan_once', description: '立即扫描一次并仅执行精确允许规则', annotations: annotations(), inputSchema: {
    type: 'object', additionalProperties: false, properties: {},
  } },
  { name: 'audit_log', description: '读取仅保存在本机的最近审计记录', annotations: annotations(true), inputSchema: {
    type: 'object', additionalProperties: false, properties: {
      limit: { type: 'integer', minimum: 1, maximum: 100, default: 20 },
    },
  } },
];

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname !== '/mcp') return new Response('Not found', { status: 404 });
    if (request.method === 'DELETE') return new Response(null, { status: 204 });
    if (request.method !== 'POST') return new Response('Method not allowed', { status: 405 });
    const rpc = await request.json() as any;
    if (rpc.method === 'notifications/initialized') return new Response(null, { status: 202 });
    if (rpc.method === 'initialize') return reply(rpc.id, {
      protocolVersion: '2025-06-18', capabilities: { tools: {}, resources: {} },
      serverInfo: { name: '__PLUGIN_NAME__', version: '0.1.0' },
    });
    if (rpc.method === 'tools/list') return reply(rpc.id, { tools });
    if (rpc.method === 'tools/call' && rpc.params?.name === 'home') return reply(rpc.id, {
      content: [{ type: 'text', text: HOME.welcome?.markdown ?? '' }], structuredContent: HOME,
    });
    if (rpc.method === 'tools/call') {
      const name = String(rpc.params?.name ?? '');
      const args = rpc.params?.arguments ?? {};
      if (name === 'start') {
        const rules = Array.isArray(args.rules) ? args.rules : [];
        if (rules.length < 1 || rules.length > 20 || rules.some((rule: any) => {
          const parts = [rule?.application, rule?.action, rule?.resource]
            .map(value => String(value ?? '').trim());
          return parts.some(value => !value || value === '*' || value === '.*' || value.length > 256);
        })) {
          return error(rpc.id, -32602, 'rules 必须包含 1-20 条 application/action/resource 精确规则');
        }
        return hostResult(rpc.id, 'desktop.chatgpt-approvals.start', {
          rules, intervalMs: args.intervalMs ?? 750,
        }, 'required');
      }
      if (name === 'stop') return hostResult(
        rpc.id, 'desktop.chatgpt-approvals.stop', {}, 'required');
      if (name === 'status') return hostResult(
        rpc.id, 'desktop.chatgpt-approvals.status', {}, 'none');
      if (name === 'scan_once') return hostResult(
        rpc.id, 'desktop.chatgpt-approvals.scan-once', {}, 'required');
      if (name === 'audit_log') return hostResult(
        rpc.id, 'desktop.chatgpt-approvals.audit', { limit: args.limit ?? 20 }, 'none');
    }
    if (rpc.method === 'resources/list') return reply(rpc.id, { resources: Object.keys(RESOURCES).map(uri => ({
      uri, name: uri.split('/').at(-1), mimeType: 'text/markdown',
    })) });
    if (rpc.method === 'resources/read') {
      const text = RESOURCES[String(rpc.params?.uri ?? '')];
      if (text === undefined) return error(rpc.id, -32002, 'Resource not found');
      return reply(rpc.id, { contents: [{ uri: rpc.params.uri, mimeType: 'text/markdown', text }] });
    }
    return error(rpc.id ?? null, -32601, 'Method not found');
  },
};
"#;

const DESKTOP_APPROVAL_TEST: &str = r#"import assert from 'node:assert/strict';
import test from 'node:test';
import worker from '../worker/src/index.ts';

const call = async (name, args = {}) => {
  const response = await worker.fetch(new Request('https://example.test/mcp', {
    method: 'POST', body: JSON.stringify({ jsonrpc: '2.0', id: 9, method: 'tools/call',
      params: { name, arguments: args } }),
  }));
  return response.json();
};

test('start emits a scoped desktop host request', async () => {
  const result = await call('start', { rules: [{
    application: 'GitHub', action: 'Enable auto-merge', resource: 'bhrumom/fabushi',
  }] });
  assert.equal(result.result.structuredContent.hostRequest.capability,
    'desktop.chatgpt-approvals.start');
  assert.equal(result.result.structuredContent.hostRequest.approval, 'required');
});

test('start rejects broad wildcard rules', async () => {
  const result = await call('start', { rules: [{ application: 'GitHub', action: '*', resource: 'all' }] });
  assert.equal(result.error.code, -32602);
});

test('status is a read-only host request', async () => {
  const result = await call('status');
  assert.equal(result.result.structuredContent.hostRequest.approval, 'none');
});
"#;

const LOCAL_STDIO_SERVER_SOURCE: &str = r#"import readline from 'node:readline';
import worker from '../worker/src/index.ts';

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  let rpc;
  try {
    rpc = JSON.parse(line);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: null,
      error: { code: -32700, message: String(error) } })}\n`);
    continue;
  }
  const response = await worker.fetch(new Request('https://standalone.invalid/mcp', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(rpc),
  }));
  if (rpc.id === undefined || response.status === 202 || response.status === 204) continue;
  const body = await response.text();
  if (body.trim()) process.stdout.write(`${body}\n`);
}
"#;

const STANDALONE_RUNTIME_TEST: &str = r#"import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import test from 'node:test';

test('standalone stdio runtime initializes without a chat host', async t => {
  const child = spawn(process.execPath,
    ['--experimental-strip-types', 'server/index.mjs'],
    { stdio: ['pipe', 'pipe', 'inherit'] });
  t.after(() => child.kill());
  const lines = readline.createInterface({ input: child.stdout, crlfDelay: Infinity });
  child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize',
    params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'test', version: '1' } } })}\n`);
  const response = JSON.parse((await lines[Symbol.asyncIterator]().next()).value);
  assert.equal(response.id, 1);
  assert.equal(response.result.protocolVersion, '2025-06-18');
  assert.equal(typeof response.result.serverInfo.name, 'string');
});
"#;

const CONTENT_COMPILER_SCRIPT: &str = r#"import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';

const plugin = JSON.parse(await fs.readFile('.codex-plugin/plugin.json', 'utf8'));
const kinds = [['tips', 'tip'], ['announcements', 'announcement'], ['articles', 'article']];
const read = async file => {
  const source = (await fs.readFile(file, 'utf8')).replaceAll('\r\n', '\n');
  const match = source.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/);
  if (!match) throw new Error(`${file} requires YAML front matter`);
  const meta = Object.fromEntries(match[1].split('\n').filter(Boolean).map(line => {
    const at = line.indexOf(':');
    const key = line.slice(0, at).trim();
    let value = line.slice(at + 1).trim().replace(/^['"]|['"]$/g, '');
    if (value.startsWith('[')) value = value.slice(1, -1).split(',').map(item => item.trim()).filter(Boolean);
    return [key, value];
  }));
  if (!meta.id || !meta.revision) throw new Error(`${file} requires id and revision`);
  return { meta, markdown: match[2].trim() };
};
const welcome = await read('content/welcome.md');
const tips = []; const items = []; const resources = {};
for (const [folder, kind] of kinds) {
  const directory = `content/${folder}`;
  for (const name of (await fs.readdir(directory).catch(() => [])).filter(name => name.endsWith('.md')).sort()) {
    const content = await read(path.join(directory, name));
    if (kind === 'tip') tips.push({ id: content.meta.id, revision: content.meta.revision, markdown: content.markdown });
    else {
      if (!content.meta.title || !content.meta.publishedAt) throw new Error(`${name} requires title and publishedAt`);
      const uri = `mahayana://${plugin.name}/content/${folder}/${content.meta.id}`;
      resources[uri] = content.markdown;
      items.push({ id: content.meta.id, revision: content.meta.revision, kind, title: content.meta.title,
        publishedAt: content.meta.publishedAt, summary: content.meta.summary || undefined,
        expiresAt: content.meta.expiresAt || undefined, coverImage: content.meta.coverImage || undefined,
        tags: content.meta.tags || [], quickReplies: [], resourceUri: uri });
    }
  }
}
items.sort((a, b) => b.publishedAt.localeCompare(a.publishedAt) || a.id.localeCompare(b.id));
const source = JSON.stringify({ welcome, tips, items, resources });
const home = { schema: 'mahayana.miniapp.home.v1', revision: crypto.createHash('sha256').update(source).digest('hex'),
  app: { id: plugin.name, title: plugin.interface.displayName, version: plugin.version, source: plugin.repository },
  welcome: { id: welcome.meta.id, markdown: welcome.markdown }, tips, quickReplies: [],
  feed: { items: items.slice(0, 10), nextCursor: items.length > 10 ? '10' : null } };
if (Buffer.byteLength(JSON.stringify(home)) > 32768) throw new Error('home payload exceeds 32 KiB');
await fs.writeFile('worker/src/content.generated.ts', `export const HOME = ${JSON.stringify(home, null, 2)} as const;\nexport const RESOURCES: Record<string,string> = ${JSON.stringify(resources, null, 2)};\n`);
"#;
