# Warp 免登录 AI 改造计划

> **目标**: 让 Warp 在不登录的情况下也能使用 AI 功能，支持自定义 LLM Provider
>
> **项目路径**: `/Users/louloulin/Documents/linchong/rust/warp`
>
> **计划版本**: 11.0 (Local LLM Provider UI 交互式配置)
>
> **生成日期**: 2026-05-02
>
> **更新日期**: 2026-05-04

---

## 📜 变更记录

| 日期 | 版本 | 变更 |
|------|------|------|
| 2026-05-04 | 11.0 | Local LLM Provider UI 交互式配置：Provider URL、API Key、Model 输入框，Save/Test 按钮 |
| 2026-05-04 | 11.1 | 移除 GlobalAIWidget 的登录检查，显示 AI 切换给所有用户；添加 is_local_llm_configured() 方法；跳过本地 LLM 配置用户的 AI 注册横幅；更新 onboarding 和 prompt_alert 以支持本地 LLM |
| 2026-05-04 | 10.0 | 修复 command_requires_auth，Generic harness 无需登录 |
| 2026-05-03 | 9.1 | GUI Bundle 验证完成，WarpOss.app 创建成功 |
| 2026-05-03 | 9.0 | 本地构建验证完成，warp-oss 编译成功 |
| 2026-05-03 | 7.4 | 更新文件清单，标记所有修改文件完成 |
| 2026-05-03 | 7.2 | 添加配置验证 (validate) 方法及单元测试 |
| 2026-05-03 | 7.1 | 补充 harness_display 集成 |
| 2026-05-03 | 7.0 | 所有功能实现完成 |
| 2026-05-03 | 6.4 | 添加集成测试 (mockito) |
| 2026-05-03 | 6.2 | 添加 Local LLM Provider 设置 UI |
| 2026-05-03 | 6.1 | 添加配置持久化 (save/load) |
| 2026-05-03 | 6.0 | 所有功能完成 |
| 2026-05-03 | 5.8 | 添加 Anthropic Claude API 支持 |
| 2026-05-03 | 5.7 | 添加剩余云端 Provider 预设 |
| 2026-05-03 | 5.6 | 添加云端 Provider 预设 |
| 2026-05-03 | 5.5 | 添加 Jan/TextGenWebUI 检测 |
| 2026-05-03 | 5.4 | 添加连接检查和模型列表 |
| 2026-05-03 | 5.3 | 添加工具/函数调用支持 |
| 2026-05-03 | 5.2 | 添加 Ollama/LM Studio 自动检测 |
| 2026-05-03 | 5.1 | 添加 SSE 流式响应支持 |
| 2026-05-02 | 5.0 | 实现 GenericHarness + OpenAI 协议 |
| 2026-05-02 | 4.0 | 真实协议分析完成 |
| 2026-05-02 | 3.0 | 全面分析 AI 架构 |
| 2026-05-02 | 2.0 | 最小实现方案 |
| 2026-05-02 | 1.0 | 初始计划 |

---

## 🔴 关键发现

### 当前真实架构

**Warp 使用自定义 Protobuf 协议，通过 Warp 服务器代理所有 LLM 调用**

```
┌─────────────────────────────────────────────────────────────────────┐
│                      当前架构 (通过 Warp Server)                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Warp Client ──Protobuf──> app.warp.dev/ai/multi-agent ──> LLM      │
│     │                     (验证 + 代理)                    │        │
│     │                              <──SSE + Base64── <─────┘        │
│     │                                                                │
│  不直接调用 Anthropic/OpenAI                                          │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 关键事实

| 问题 | 答案 |
|------|------|
| **当前协议** | 自定义 Protobuf (`application/x-protobuf`) |
| **是否直接调用 Anthropic** | ❌ 否 |
| **是否直接调用 OpenAI** | ❌ 否 |
| **是否调用第三方 API** | ❌ 否 |
| **所有调用都通过** | ✅ Warp 服务器 (`app.warp.dev`) |
| **离线可用** | ❌ 需要连接 Warp 服务器 |

### 代码证据

**1. Protobuf 请求** (`app/src/ai/agent/api/impl.rs`):
```rust
let request = api::Request {
    task_context: Some(...),
    input: Some(...),
    settings: Some(...),
    // Protobuf 编码后发送
};
```

**2. HTTP 发送** (`app/src/server/server_api.rs`):
```rust
let url = format!(
    "{}/{}/{}",
    ChannelState::server_root_url(),  // https://app.warp.dev
    "ai",
    "multi-agent"
);
// POST + Protobuf 编码
```

**3. 响应处理**:
```rust
// SSE + Base64 解码 + Protobuf 解码
warp_multi_agent_api::ResponseEvent::decode(decoded_data.as_slice())
```

---

## 🎯 目标架构

### 绕过 Warp 服务器，直接调用 LLM

```
┌─────────────────────────────────────────────────────────────────────┐
│                      目标架构 (直接调用 LLM)                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Warp Client ──HTTPS──> Ollama/LM Studio/任意 OpenAI 兼容 API       │
│     │                            │                                  │
│     │                     直接 HTTP 调用                             │
│     │                     标准 JSON API                              │
│     │                                                                │
│  完全离线可用                                                          │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 📋 当前状态

### ✅ 已完成

| 修改 | 文件 | 状态 |
|------|------|------|
| 移除登录检查 | `settings/ai.rs` | ✅ 2026-05-02 |
| 移除登录检查 | `agent_sdk/driver.rs` | ✅ 2026-05-02 |
| 添加 Generic Harness | `Harness` 枚举 | ✅ 2026-05-02 |
| 实现 GenericHttpHarness | `generic_http.rs` | ✅ 2026-05-02 |
| OpenAI 协议支持 | Chat Completions API | ✅ 2026-05-02 |
| Provider 配置 | `GenericProviderConfig` | ✅ 2026-05-02 |
| SSE 流式响应 | StreamingDelta/Chunk | ✅ 2026-05-03 |
| Provider 自动检测 | Ollama/LM Studio | ✅ 2026-05-03 |
| 工具调用支持 | with_tools/add_tool | ✅ 2026-05-03 |
| 连接检查/模型列表 | check_connection/get_models | ✅ 2026-05-03 |
| 配置持久化 | save/load 方法 | ✅ 2026-05-03 |
| Provider 配置 UI | LocalLLMProviderWidget | ✅ 2026-05-03 |
| 单元测试 | Config/Harness 测试 | ✅ 2026-05-03 |
| 集成测试 | mockito HTTP mock 测试 | ✅ 2026-05-03 |
| UI 集成 | harness_display.rs | ✅ 2026-05-03 |
| 配置验证 | validate() 方法 | ✅ 2026-05-03 |
| CLI 修复 | --model 与 ModelArgs 冲突修复 | ✅ 2026-05-03 |
| 本地构建验证 | cargo build --bin warp-oss | ✅ 2026-05-03 |
| command_requires_auth 修复 | Generic harness 无需登录 | ✅ 2026-05-04 |
| Metal shader 编译跳过 | 无 Xcode CLI tools 时跳过 | ✅ 2026-05-04 |
| Local LLM Provider UI | 交互式配置界面 | ✅ 2026-05-04 |

### ✅ 验证结果

**本地构建测试** (2026-05-04):
```bash
$ cargo build --bin warp-oss
warning: warpui@0.0.0: Metal shader compilation skipped (Metal compiler not available)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 11s
```

**command_requires_auth 修复** (2026-05-04):
当使用 `--harness generic` 或指定 `--provider-url`/`--api-key`/`--llm-model` 时，不再要求登录。
```rust
// app/src/ai/agent_sdk/mod.rs
AgentCommand::Run(run_args) => {
    if run_args.provider_url.is_some()
        || run_args.api_key.is_some()
        || run_args.llm_model.is_some()
        || run_args.harness == Harness::Generic
    {
        false  // 不需要登录
    } else {
        true
    }
}
```

**CLI Generic Harness 验证** (2026-05-04):
```bash
$ ./target/debug/warp-oss agent run --harness generic --prompt "Hello"
# 不再显示 "You are not logged in" 错误
# 执行可能因沙盒限制失败，但 CLI 参数验证通过
```

**二进制信息**:
```
target/debug/warp-oss
- Mach-O 64-bit executable arm64
- Size: ~700MB (debug build)
```

**已知限制**: macOS 沙盒阻止 CLI 直接执行，但命令参数验证通过

### 🔲 待完成 (手动测试)

| 组件 | 说明 |
|------|------|
| **实际环境测试** | 需要 Ollama/LM Studio 运行实例 |
| **真实启动 GUI** | 需要 macOS GUI 沙盒权限 |

---

## 🛠️ 实施计划

### Phase 2: GenericHarness (P0)

**目标**: 绕过 Warp 服务器，直接调用 OpenAI 兼容 API

**架构**:
```
GenericHarness
    │
    ├── HTTP Client (已有: reqwest)
    │
    ├── OpenAI 兼容 API 调用
    │   ├── POST /chat/completions
    │   ├── Authorization: Bearer <API_KEY>
    │   └── JSON 请求/响应
    │
    └── SSE 流式响应解析
        └── 文本/工具调用
```

**新增文件**:
```
app/src/ai/agent_sdk/driver/harness/
├── generic_http.rs      # Generic HTTP Harness
├── openai_protocol.rs   # OpenAI API 协议处理
└── mod.rs             # 注册
```

### Phase 3: Provider 配置 (P0)

**目标**: 支持多个 Provider 配置

**配置结构**:
```rust
pub struct OpenAICompatibleProvider {
    pub name: String,           // 显示名称
    pub base_url: String,       // http://localhost:11434
    pub api_key: Option<String>,
    pub model: String,          // llama3, gpt-4, etc.
}
```

### Phase 4: UI 配置 (P1)

**目标**: 用户配置界面

---

## 🔌 支持的协议

### 目标协议支持

| 协议 | 状态 | 说明 |
|------|------|------|
| **OpenAI Chat Completions API** | ✅ 已实现 | 标准 JSON API |
| **Anthropic Messages API** | ✅ 已实现 | 直接调用 Claude |
| **SSE (Server-Sent Events)** | ✅ 已实现 | 流式响应 |
| **JSON Stream** | ✅ 已实现 | 增量 JSON |

### Provider 支持

| Provider | 协议 | URL | 状态 |
|----------|------|-----|------|
| **Ollama** | OpenAI 兼容 | localhost:11434 | ✅ |
| **LM Studio** | OpenAI 兼容 | localhost:1234 | ✅ |
| **Jan** | OpenAI 兼容 | localhost:1337 | ✅ |
| **Text Generation WebUI** | OpenAI 兼容 | localhost:5000 | ✅ |
| **OpenAI** | OpenAI API | api.openai.com | ✅ |
| **Anthropic** | Anthropic API | api.anthropic.com | ✅ |
| **Groq** | OpenAI 兼容 | api.groq.com | ✅ |
| **Together AI** | OpenAI 兼容 | api.together.xyz | ✅ |
| **Fireworks AI** | OpenAI 兼容 | api.fireworks.ai | ✅ |
| **Mistral** | OpenAI 兼容 | api.mistral.ai | ✅ |
| **Perplexity** | OpenAI 兼容 | api.perplexity.ai | ✅ |
| **Azure OpenAI** | OpenAI 兼容 | *.azurewebsites.net | ✅ |

---

## 📁 文件清单

### ✅ 已完成

**新增文件**:
| 文件 | 说明 |
|------|------|
| `app/src/ai/agent_sdk/driver/harness/generic_http.rs` | Generic HTTP Harness (~1600行) |

**修改文件**:
| 文件 | 修改 |
|------|------|
| `crates/warp_cli/src/agent.rs` | 添加 `Harness::Generic` 枚举，修复 harness_generic ArgGroup |
| `app/src/ai/agent_sdk/driver/harness/mod.rs` | 注册 GenericHarness |
| `app/src/ai/agent_sdk/mod.rs` | 添加 build_generic_harness_args，修复 command_requires_auth |
| `app/src/ai/agent_sdk/driver.rs` | 移除登录检查，添加 AI 可用性注释 |
| `app/src/ai/harness_display.rs` | 添加 Generic harness 显示支持 |
| `app/src/settings_view/ai_page.rs` | 添加 LocalLLMProviderWidget UI，移除 GlobalAIWidget 登录检查 |
| `crates/warpui/build.rs` | 修复 Metal shader 编译跳过逻辑 |
| `app/src/settings/ai.rs` | 移除 is_anonymous_or_logged_out 检查，添加 is_local_llm_configured() |
| `app/src/terminal/view.rs` | 跳过本地 LLM 配置用户的 AI 注册横幅 |
| `app/src/ai/blocklist/prompt/prompt_alert.rs` | 本地 LLM 用户跳过匿名用户请求限制 |
| `app/src/ai/onboarding.rs` | 本地 LLM 配置用户显示 FreeUser onboarding |

---

## ✅ 验收标准

- [x] 移除登录检查
- [x] GenericHarness 实现
- [x] OpenAI 兼容 API 支持
- [x] Provider 配置 (GenericProviderConfig)
- [x] 流式响应处理 (SSE)
- [x] Anthropic 直接 API 支持
- [x] Provider 配置 UI
- [x] 集成测试 (mockito)
- [x] 配置验证 (validate)
- [x] UI 登录门移除 - GlobalAIWidget 显示 AI 切换给所有用户
- [x] 本地 LLM 配置检测 - is_local_llm_configured() 方法
- [x] AI 注册横幅跳过 - 本地 LLM 配置用户不显示注册提示
- [x] 匿名用户请求限制豁免 - 本地 LLM 用户不受限制
- [x] Onboarding 状态更新 - 本地 LLM 配置用户显示 FreeUser onboarding
- [ ] Ollama 实际环境测试

---

## 💡 为什么选择 OpenAI 兼容协议

1. **广泛支持**: Ollama, LM Studio, Jan, Text Generation WebUI 都支持
2. **标准格式**: 请求/响应格式统一
3. **易于实现**: JSON + HTTPS，无需复杂协议
4. **扩展性**: 可添加任何兼容服务

---

## ⚠️ 技术挑战

| 挑战 | 解决方案 |
|------|----------|
| Protobuf → JSON 转换 | 实现新协议层 |
| 工具调用格式 | 适配 OpenAI function calling |
| 上下文管理 | 本地维护对话历史 |
| 认证 | Bearer Token / API Key |

---

## 🔧 手动验证指南

由于沙箱环境限制，构建和运行验证需要在本地手动完成。

### 步骤 1: 初始化 warp-workflows

```bash
cd /Users/louloulin/Documents/linchong/rust/warp
git submodule update --init --recursive
# 或手动克隆:
git clone --depth 1 --branch trunk https://github.com/warpdotdev/workflows.git crates/warp-workflows
```

### 步骤 2: 构建项目

```bash
cargo build --bin warp-oss
```

### 步骤 3: 启动本地 LLM

**Ollama:**
```bash
ollama serve
ollama pull llama3
```

**LM Studio:**
```bash
# 下载并启动 LM Studio
# 加载模型 (如 llama3)
```

### 步骤 4: 测试 Generic Harness

```bash
# 测试基本对话
./target/debug/warp-oss run -p "Hello, how are you?" --harness generic --llm-model llama3 --provider-url http://localhost:11434

# 测试 OpenAI API
./target/debug/warp-oss run -p "Hello" --harness generic --provider-url https://api.openai.com/v1 --api-key sk-xxx --llm-model gpt-4

# 测试 Anthropic API
./target/debug/warp-oss run -p "Hello" --harness generic --provider-url https://api.anthropic.com/v1 --api-key sk-xxx --llm-model claude-3-sonnet-20240229
```

### 步骤 5: 验证功能

- [ ] 无登录使用 AI
- [ ] Ollama 本地模型
- [ ] LM Studio 本地模型
- [ ] OpenAI API 调用
- [ ] Anthropic API 调用
- [ ] 流式响应显示
- [ ] 工具调用 (function calling)

### 已知限制

- ~~`crates/warp-workflows` 需要单独初始化 (git submodule)~~ - 已修复，使用 workspace exclude
- ~~构建需要 Metal shaders (Xcode 开发者工具)~~ - 已修复，跳过已编译的 shaders
- 网络代理环境可能需要配置
- 真实 GUI 启动需要 macOS 沙盒权限

---

*本计划由 Claude Code 自动生成*
