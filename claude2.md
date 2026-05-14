# Warp AI Agent 架构分析文档

> **版本**: v2.0
> **日期**: 2026-05-14
> **项目路径**: `/Users/louloulin/Documents/linchong/rust/warp`

---

## 目录

1. [整体架构概览](#1-整体架构概览)
2. [核心模块](#2-核心模块)
3. [数据流](#3-数据流)
4. [事件协议](#4-事件协议)
5. [Harness 系统](#5-harness-系统)
6. [Local LLM 实现](#6-local-llm-实现)
7. [UI 渲染](#7-ui-渲染)
8. [API 协议](#8-api-协议)

---

## 1. 整体架构概览

### 1.1 系统架构图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Warp Application                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────┐    ┌─────────────────┐    ┌────────────────────────────┐ │
│  │   UI Layer │    │  BlocklistAI    │    │    AgentConversations      │ │
│  │  (warpui)  │◄──►│  Controller     │◄──►│    Model                   │ │
│  └─────────────┘    └─────────────────┘    └────────────────────────────┘ │
│         │                  │                        │                      │
│         │                  ▼                        ▼                      │
│         │         ┌─────────────────┐    ┌────────────────────────────┐ │
│         │         │  ResponseStream │    │   HistoryModel             │ │
│         │         │  Handler        │    │   (Conversation State)      │ │
│         │         └─────────────────┘    └────────────────────────────┘ │
│         │                  │                                               │
│         ▼                  ▼                                               │
│  ┌─────────────────────────────────────────────────────────────┐           │
│  │                    AI Agent SDK                             │           │
│  │  ┌─────────────────────────────────────────────────────┐   │           │
│  │  │              AgentDriver                             │   │           │
│  │  │  ┌───────────┬───────────┬───────────┬──────────┐  │   │           │
│  │  │  │ClaudeHarn │GenericHarn│GeminiHarn │LocalLLM  │  │   │           │
│  │  │  │   ess     │    ess    │   ess     │ Harness  │  │   │           │
│  │  │  └───────────┴───────────┴───────────┴──────────┘  │   │           │
│  │  └─────────────────────────────────────────────────────┘   │           │
│  └─────────────────────────────────────────────────────────────┘           │
│                              │                                               │
└──────────────────────────────│───────────────────────────────────────────────┘
                               │
                               ▼
              ┌────────────────────────────────────┐
              │      External Services             │
              │  ┌──────────┐  ┌───────────────┐  │
              │  │Warp Server│  │LLM Providers  │  │
              │  │(GraphQL)  │  │(OpenAI/Anthropic│ │
              │  └──────────┘  │ Claude Code)  │  │
              │                 └───────────────┘  │
              └────────────────────────────────────┘
```

### 1.2 模块层次

```
app/src/ai/
├── mod.rs                 # AI 模块入口
├── agent/                # Agent 核心逻辑
│   ├── conversation.rs    # 对话状态管理
│   ├── task.rs           # 任务管理
│   ├── task_store.rs     # 任务存储
│   └── api/              # API 层
│       ├── mod.rs        # API 入口
│       └── local_llm_output.rs  # 本地 LLM 输出
│
├── agent_sdk/            # Agent SDK
│   ├── mod.rs           # SDK 入口 (~64KB)
│   ├── driver.rs        # 驱动核心 (~98KB)
│   ├── provider.rs      # Provider 配置
│   ├── output.rs       # 输出处理
│   └── driver/
│       ├── harness/
│       │   ├── claude_code.rs    # Claude Code Harness
│       │   ├── generic_http.rs   # 通用 HTTP Harness
│       │   └── gemini.rs         # Gemini Harness
│       └── terminal.rs           # 终端驱动
│
├── blocklist/           # Blocklist UI 组件
│   ├── controller.rs   # AI 控制器 (~117KB)
│   ├── history_model.rs # 历史记录模型
│   ├── context_model.rs # 上下文模型
│   ├── input_model.rs   # 输入模型
│   ├── block.rs         # Block 渲染 (~265KB)
│   └── block/           # Block 子组件
│       ├── cli.rs       # CLI Block
│       └── ...
│
├── llms.rs             # LLM 配置 (~39KB)
├── onboarding.rs       # onboarding 逻辑
└── ...
```

---

## 2. 核心模块

### 2.1 BlocklistAIController

**文件**: `app/src/ai/blocklist/controller.rs` (~117KB)

```
┌─────────────────────────────────────────────────────────────┐
│              BlocklistAIController                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────────────┐    ┌──────────────────┐              │
│  │  InputContext   │───►│  RequestBuilder  │              │
│  └──────────────────┘    └────────┬─────────┘              │
│                                    │                         │
│                                    ▼                         │
│                          ┌──────────────────┐              │
│                          │  ResponseStream  │              │
│                          │    Handler       │              │
│                          └────────┬─────────┘              │
│                                   │                        │
│          ┌───────────────────────┼───────────────────────┐│
│          │                       │                       ││
│          ▼                       ▼                       ▼│
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐│
│  │ HistoryModel │    │ ContextModel │    │InputModel   ││
│  └──────────────┘    └──────────────┘    └──────────────┘│
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**核心职责**:
- 接收用户输入并构建请求
- 管理响应流处理
- 协调 UI 更新
- 处理认证和权限

### 2.2 AgentDriver

**文件**: `app/src/ai/agent_sdk/driver.rs` (~98KB)

```
┌─────────────────────────────────────────────────────────────┐
│                      AgentDriver                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              ThirdPartyHarness Trait                │    │
│  │  + harness() -> Harness                             │    │
│  │  + cli_agent() -> CLIAgent                        │    │
│  │  + validate() -> Result                            │    │
│  │  + prepare_environment_config()                     │    │
│  │  + fetch_resume_payload()                          │    │
│  │  + run_chat(harness, input) -> ResponseStream     │    │
│  └─────────────────────────────────────────────────────┘    │
│                              │                              │
│           ┌──────────────────┼──────────────────┐          │
│           │                  │                  │          │
│           ▼                  ▼                  ▼          │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐ │
│  │ClaudeHarness │    │GenericHttp  │    │GeminiHarness│ │
│  │              │    │Harness      │    │              │ │
│  │ - Claude CLI │    │ - HTTP API  │    │ - Gemini API │ │
│  │ - Resume     │    │ - OpenAI    │    │ - Vertex AI │ │
│  │ - Transcript │    │ - Anthropic │    │              │ │
│  └──────────────┘    │ - LocalLLM  │    └──────────────┘ │
│                      └──────────────┘                       │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 2.3 ResponseStream

**文件**: `app/src/ai/blocklist/controller/response_stream.rs`

```
┌─────────────────────────────────────────────────────────────┐
│                    ResponseStream                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   Event Types:                                               │
│   ┌─────────────────────────────────────────────────────┐   │
│   │  Type::Init(StreamInit)                            │   │
│   │    - conversation_id                                 │   │
│   │    - request_id                                      │   │
│   │    - run_id                                          │   │
│   ├─────────────────────────────────────────────────────┤   │
│   │  Type::ClientActions(ClientActions)                  │   │
│   │    - CreateTask                                      │   │
│   │    - AppendToMessageContent                          │   │
│   │    - ShowSuggestions                                 │   │
│   │    - MoveMessagesToNewTask                           │   │
│   ├─────────────────────────────────────────────────────┤   │
│   │  Type::Finished(StreamFinished)                      │   │
│   │    - reason: Done | InternalError | MaxTokens        │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. 数据流

### 3.1 用户请求流程

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          用户请求流程                                        │
└─────────────────────────────────────────────────────────────────────────────┘

  User Input
      │
      ▼
┌─────────────┐
│ AgentInput  │  用户输入 "解释这段代码"
│ Model       │
└──────┬──────┘
       │
       ▼
┌─────────────────────────────┐
│ BlocklistAIController       │
│ input_model.handle_input() │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ build_request()             │
│ - InputContext              │
│ - SystemPrompt              │
│ - Tools                    │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ AgentDriver::run_chat()    │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ ThirdPartyHarness          │
│ (ClaudeHarness/GeneircHttp)│
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐     ┌─────────────────┐
│ ResponseStream              │────►│ Warp Server     │
│ (SSE Events)               │     │ (Optional)      │
└──────┬──────────────────────┘     └─────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ ResponseStreamHandler       │
│ - Parse Events             │
│ - Emit to UI              │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ BlocklistAIHistoryModel     │
│ - Update Task              │
│ - Update Exchange           │
│ - Emit Events              │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ UI Layer (warpui)         │
│ - Render Message          │
│ - Show Suggestions        │
└─────────────────────────────┘
```

### 3.2 Local LLM 请求流程

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                       Local LLM 请求流程                                      │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────────────────────────────────────────────────────────────┐
  │                         Local LLM Flow                                │
  └──────────────────────────────────────────────────────────────────────┘

  User Input
      │
      ▼
  BlocklistAIController
      │
      ▼
  AgentDriver::run_chat()
      │
      ├──► is_local_llm_configured()?
      │         │
      │         Yes
      │         │
      │         ▼
      │    ┌────────────────────┐
      │    │ local_llm_output.rs│
      │    │ generate_local_    │
      │    │ llm_output()       │
      │    └─────────┬──────────┘
      │              │
      │              ▼
      │    ┌────────────────────┐
      │    │ GenericProviderConfig│
      │    │ - base_url         │
      │    │ - api_key          │
      │    │ - default_model    │
      │    │ - api_format       │  ◄── OpenAI / Anthropic
      │    └─────────┬──────────┘
      │              │
      │              ▼
      │    ┌────────────────────┐
      │    │ reqwest::Client    │
      │    │ POST /v1/chat/     │
      │    │ completions        │
      │    └─────────┬──────────┘
      │              │
      │              ▼
      │    ┌────────────────────┐
      │    │ SSE Stream Response│
      │    │ (OpenAI or        │
      │    │  Anthropic format)│
      │    └─────────┬──────────┘
      │              │
      │              ▼
      │    ┌────────────────────┐
      │    │ Parse SSE Events  │
      │    │ - event: prefix   │
      │    │ - data: JSON      │
      │    └─────────┬──────────┘
      │              │
      │              ▼
      │    ┌────────────────────┐
      │    │ Convert to        │
      │    │ ResponseEvent     │
      │    │ - Init            │
      │    │ - ClientActions   │  ◄── AppendToMessageContent
      │    │ - Finished        │
      │    └─────────┬──────────┘
      │              │
      │
      No (Cloud path)
      │
      ▼
  ┌─────────────────────────────────────────────────┐
  │ warp_multi_agent_api::Client                    │
  │ ServerApi.send_chat_request()                   │
  └─────────────────────────────────────────────────┘
```

---

## 4. 事件协议

### 4.1 ResponseEvent 类型

```rust
// warp_multi_agent_api::response_event::Type

pub enum Type {
    /// Stream initialization
    Init(StreamInit {
        conversation_id: String,
        request_id: String,
        run_id: String,
    }),

    /// Client actions (CreateTask, AppendToMessageContent, etc.)
    ClientActions(ClientActions {
        actions: Vec<ClientAction>,
    }),

    /// Stream finished
    Finished(StreamFinished {
        reason: Option<Reason>,
    }),
}
```

### 4.2 ClientAction 类型

```rust
// warp_multi_agent_api::client_action::Action

pub enum Action {
    /// Create new task and associated exchange
    CreateTask(CreateTask {
        task: Option<Task>,
    }),

    /// Append content to existing message
    AppendToMessageContent(AppendToMessageContent {
        task_id: String,
        message: Option<Message>,
        mask: Option<PermissionMask>,
    }),

    /// Show suggestions
    ShowSuggestions(Vec<Suggestion>),

    /// Move messages to new task
    MoveMessagesToNewTask(MoveMessagesToNewTask {
        source_task_id: String,
        new_task: Option<Task>,
        first_message_id: String,
        last_message_id: String,
        expected_message_count: usize,
        replacement_messages: Vec<Message>,
    }),

    /// Start new conversation
    StartNewConversation(StartNewConversation),
}
```

### 4.3 Local LLM 事件流问题

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     Local LLM 当前事件流 (有问题)                              │
└─────────────────────────────────────────────────────────────────────────────┘

Expected Flow:                    Actual Flow:
┌─────────────────┐              ┌─────────────────┐
│ Init            │              │ Init            │
│ (request_id)    │              │ (request_id)    │
└────────┬────────┘              └────────┬────────┘
         │                                │
         ▼                                ▼
┌─────────────────┐              ┌─────────────────┐
│ CreateTask      │              │ AppendToMessage │
│ (创建Task/Exchange)│              │ Content         │  ◄── ERROR!
└────────┬────────┘              │ (缺少前置Setup) │
         │                                │
         ▼                                ▼
┌─────────────────┐              ┌─────────────────┐
│ AppendToMessage │              │ Finished         │
│ Content         │              │                 │
└────────┬────────┘              └─────────────────┘
         │
         ▼
┌─────────────────┐
│ Finished        │
│                 │
└─────────────────┘

问题分析:
- AppendToMessageContent 需要 added_exchanges_by_response 中有对应条目
- 该条目由 CreateTask 处理时设置
- Local LLM 直接发送 AppendToMessageContent，缺少 CreateTask
```

---

## 5. Harness 系统

### 5.1 Harness 继承层次

```
┌─────────────────────────────────────────────────────────────┐
│                    Harness 枚举                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   pub enum Harness {                                         │
│       Claude,         // Claude Code CLI                    │
│       Gemini,         // Google Gemini                      │
│       OpenAI,         // OpenAI API (via HTTP)             │
│       AzureOpenAI,    // Azure OpenAI                      │
│       CustomCLIAgent, // Custom CLI Agent                  │
│   }                                                          │
│                                                              │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ ThirdPartyHarness Trait
                              ▼
┌─────────────────────────────────────────────────────────────┐
│              ThirdPartyHarness Trait                        │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   pub trait ThirdPartyHarness: Send + Sync {               │
│       fn harness(&self) -> Harness;                         │
│       fn cli_agent(&self) -> CLIAgent;                     │
│       fn install_docs_url(&self) -> Option<&'static str>;   │
│       fn validate(&self) -> Result<(), AgentDriverError>;  │
│       fn prepare_environment_config(...)                    │
│           -> Result<(), AgentDriverError>;                 │
│       fn fetch_resume_payload(...)                         │
│           -> Result<Option<ResumePayload>, ...>;           │
│       async fn run_chat(...)                               │
│           -> Result<ResponseStream, ...>;                   │
│   }                                                          │
│                                                              │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌──────────────────┼──────────────────┐
          │                  │                  │
          ▼                  ▼                  ▼
   ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
   │ Claude      │    │ GenericHttp │    │ Gemini      │
   │ Harness     │    │ Harness     │    │ Harness     │
   │             │    │             │    │             │
   │ - CLI based │    │ - HTTP API  │    │ - REST API │
   │ - Transcript│    │ - OpenAI    │    │ - Vertex   │
   │ - Resume    │    │ - LocalLLM  │    │             │
   └─────────────┘    └─────────────┘    └─────────────┘
```

### 5.2 ClaudeHarness 实现

**文件**: `app/src/ai/agent_sdk/driver/harness/claude_code.rs`

```
┌─────────────────────────────────────────────────────────────┐
│                     ClaudeHarness                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  启动流程:                                                    │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ 1. prepare_environment_config()                       │  │
│  │    - 写入 CLAUDE.md 配置                              │  │
│  │    - 写入 .claude/config.json                         │  │
│  │    - 设置 API key 环境变量                             │  │
│  │    - 配置 parent.listen 端口                          │  │
│  └─────────────────────────────────────────────────────┘  │
│                          │                                  │
│                          ▼                                  │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ 2. spawn Claude Code CLI                              │  │
│  │    command: npx @anthropic/claude-code              │  │
│  │    args: [--print, --output-format=stream-json]     │  │
│  │    env: ANTHROPIC_API_KEY, CLAUDE_CONFIG_DIR, etc.  │  │
│  └─────────────────────────────────────────────────────┘  │
│                          │                                  │
│                          ▼                                  │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ 3. parse_stream_json()                               │  │
│  │    - 解析 Claude Code SSE 输出                        │  │
│  │    - 转换为 ResponseEvent                            │  │
│  │    - 处理 transcript 事件                             │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 GenericHttpHarness 实现

**文件**: `app/src/ai/agent_sdk/driver/harness/generic_http.rs`

```
┌─────────────────────────────────────────────────────────────┐
│                   GenericHttpHarness                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  配置结构:                                                    │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ GenericProviderConfig {                               │  │
│  │     name: String,           // "Ollama", "LM Studio" │  │
│  │     base_url: String,       // "http://localhost:11434"│  │
│  │     api_key: Option<String>,                         │  │
│  │     default_model: String,  // "llama3", "gpt-4"    │  │
│  │     streaming: bool,       // true                  │  │
│  │     api_format: ApiFormat,  // OpenAI | Anthropic   │  │
│  │ }                                                    │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
│  API Format:                                                │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ ApiFormat::OpenAI                                     │  │
│  │     - Endpoint: /v1/chat/completions                  │  │
│  │     - Body: { model, messages, stream: true }        │  │
│  │     - Response: SSE with data: [DONE]               │  │
│  ├─────────────────────────────────────────────────────┤  │
│  │ ApiFormat::Anthropic                                  │  │
│  │     - Endpoint: /v1/messages                          │  │
│  │     - Header: anthropic-version: 2023-06-01         │  │
│  │     - Body: { model, messages, stream: true }      │  │
│  │     - Response: SSE with event: prefix              │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 6. Local LLM 实现

### 6.1 文件结构

```
app/src/ai/
├── agent/
│   └── api/
│       ├── mod.rs           # API 入口
│       ├── impl.rs          # 实现 (Local LLM 路由)
│       └── local_llm_output.rs  # Local LLM 输出生成
│
└── agent_sdk/
    └── driver/
        └── harness/
            ├── mod.rs       # Harness 模块
            └── generic_http.rs  # Generic HTTP Harness
```

### 6.2 Local LLM 路由

**文件**: `app/src/ai/agent/api/impl.rs`

```
┌─────────────────────────────────────────────────────────────┐
│                   Local LLM 路由                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   fn get_response_stream(...) -> ResponseStream {           │
│                                                              │
│       // 检查是否配置了本地 LLM                               │
│       if let Some(config) = load_local_llm_config() {      │
│           // 使用本地 LLM                                    │
│           return generate_local_llm_output(               │
│               config,                                        │
│               params,                                       │
│               cancellation_rx,                               │
│           );                                                │
│       }                                                      │
│                                                              │
│       // 否则使用云端 Warp Server                          │
│       return warp_multi_agent_api::Client::send_chat(...); │
│   }                                                          │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 6.3 SSE 解析

**文件**: `app/src/ai/agent/api/local_llm_output.rs`

```
┌─────────────────────────────────────────────────────────────┐
│                    SSE 事件解析                              │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  OpenAI 格式 (默认):                                         │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ data: {"id":"chatcmpl-xxx","choices":[{"delta":     │  │
│  │ {"content":"Hello"},"finish_reason":null}]}        │  │
│  │                                                        │  │
│  │ data: {"id":"chatcmpl-xxx","choices":[{"delta":     │  │
│  │ {"content":" world"},"finish_reason":null}]}        │  │
│  │                                                        │  │
│  │ data: [DONE]                                         │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
│  Anthropic 格式 (MiniMax 等):                               │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ event: message_start                                 │  │
│  │ data: {"type":"message_start",...}                  │  │
│  │                                                        │  │
│  │ event: content_block_delta                            │  │
│  │ data: {"type":"content_block_delta","index":1,      │  │
│  │ "delta":{"type":"text_delta","text":"Hello"}}       │  │
│  │                                                        │  │
│  │ event: message_stop                                   │  │
│  │ data: {"type":"message_stop"}                         │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
│  解析逻辑:                                                    │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ 1. 读取字节流                                        │  │
│  │ 2. 按行分割 (lines)                                  │  │
│  │ 3. 检查 "event: " 前缀 → 识别事件类型                │  │
│  │ 4. 检查 "data: " 前缀 → 解析 JSON                    │  │
│  │ 5. 根据 api_format 选择解析器                         │  │
│  │    - OpenAI: OpenAIStreamingChunk                    │  │
│  │    - Anthropic: AnthropicSSEChunk                     │  │
│  │ 6. 累积文本到 text_buffer                            │  │
│  │ 7. 发送 AppendToMessageContent 事件                  │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 7. UI 渲染

### 7.1 Block 渲染层次

```
┌─────────────────────────────────────────────────────────────┐
│                    UI Block 层次                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  BlocklistAIController                                       │
│       │                                                      │
│       ▼                                                      │
│  BlocklistAIHistoryModel                                    │
│       │                                                      │
│       ├──► TerminalView (终端输出)                           │
│       ├──► UserMessageBlock (用户输入)                       │
│       ├──► AIMessageBlock (AI 响应)                         │
│       │       │                                              │
│       │       ├──► CodeBlock (代码块)                        │
│       │       ├──► TextBlock (文本块)                        │
│       │       └──► ToolUseBlock (工具调用块)                 │
│       ├──► SuggestionChip (建议)                            │
│       └──► ErrorBlock (错误信息)                            │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 7.2 Block 渲染流程

```
┌─────────────────────────────────────────────────────────────┐
│                    Block 渲染流程                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   ResponseEvent                                              │
│       │                                                      │
│       ▼                                                      │
│   ┌─────────────────────────────────────────────────────┐  │
│   │ ClientActions 处理                                    │  │
│   │ - CreateTask → 创建 Task 和 Exchange                  │  │
│   │ - AppendToMessageContent → 更新消息内容               │  │
│   └─────────────────────────────────────────────────────┘  │
│       │                                                      │
│       ▼                                                      │
│   ┌─────────────────────────────────────────────────────┐  │
│   │ HistoryModel 更新                                      │  │
│   │ - task_store.modify_task()                           │  │
│   │ - exchange.append_message()                          │  │
│   └─────────────────────────────────────────────────────┘  │
│       │                                                      │
│       ▼                                                      │
│   ┌─────────────────────────────────────────────────────┐  │
│   │ BlocklistAIHistoryEvent                              │  │
│   │ - AppendedExchange                                   │  │
│   │ - UpdatedStreamingExchange                          │  │
│   │ - UpdatedTodoList                                   │  │
│   └─────────────────────────────────────────────────────┘  │
│       │                                                      │
│       ▼                                                      │
│   ┌─────────────────────────────────────────────────────┐  │
│   │ Block.rs render()                                    │  │
│   │ - 根据 exchange type 选择渲染器                       │  │
│   │ - 生成 warpui Element                               │  │
│   └─────────────────────────────────────────────────────┘  │
│       │                                                      │
│       ▼                                                      │
│   warpui Element Tree                                       │
│       │                                                      │
│       ▼                                                      │
│   macOS/iOS View                                            │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 8. API 协议

### 8.1 Warp Multi-Agent API

**Crate**: `crates/warp-multi-agent-api/`

```
┌─────────────────────────────────────────────────────────────┐
│                Warp Multi-Agent API                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ response_event/                                       │  │
│  │ ├── Type.rs          # ResponseEvent 枚举            │  │
│  │ ├── Init.rs          # StreamInit 结构体              │  │
│  │ ├── ClientActions.rs # ClientActions 结构体           │  │
│  │ └── Finished.rs      # StreamFinished 结构体          │  │
│  ├─────────────────────────────────────────────────────┤  │
│  │ client_action/                                        │  │
│  │ ├── Action.rs        # ClientAction 枚举            │  │
│  │ ├── CreateTask.rs    # CreateTask 结构体             │  │
│  │ ├── AppendToMessageContent.rs                        │  │
│  │ └── ...                                             │  │
│  ├─────────────────────────────────────────────────────┤  │
│  │ message/                                             │  │
│  │ ├── Message.rs      # 消息结构                       │  │
│  │ ├── AgentOutput.rs  # AI 输出                       │  │
│  │ └── ...                                             │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 8.2 Protobuf 定义

```
┌─────────────────────────────────────────────────────────────┐
│                Protocol Buffer 消息                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   // Proto 定义 (简化)                                        │
│                                                              │
│   message ResponseEvent {                                    │
│       oneof type {                                          │
│           StreamInit init = 1;                              │
│           ClientActions client_actions = 2;                  │
│           StreamFinished finished = 3;                      │
│       }                                                      │
│   }                                                          │
│                                                              │
│   message StreamInit {                                       │
│       string conversation_id = 1;                            │
│       string request_id = 2;                                │
│       string run_id = 3;                                     │
│   }                                                          │
│                                                              │
│   message ClientActions {                                    │
│       repeated ClientAction actions = 1;                    │
│   }                                                          │
│                                                              │
│   message ClientAction {                                     │
│       oneof action {                                         │
│           CreateTask create_task = 1;                       │
│           AppendToMessageContent append_to_message = 2;      │
│           // ...                                            │
│       }                                                      │
│   }                                                          │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 附录 A: 关键文件索引

| 文件路径 | 大小 | 功能 |
|---------|------|------|
| `app/src/ai/blocklist/controller.rs` | ~117KB | BlocklistAIController 核心 |
| `app/src/ai/blocklist/block.rs` | ~265KB | Block 渲染 |
| `app/src/ai/blocklist/history_model.rs` | ~96KB | 历史记录模型 |
| `app/src/ai/agent_sdk/driver.rs` | ~98KB | AgentDriver 核心 |
| `app/src/ai/agent_sdk/mod.rs` | ~64KB | Agent SDK 入口 |
| `app/src/ai/agent/conversation.rs` | ~152KB | Conversation 状态管理 |
| `app/src/ai/agent_sdk/driver/harness/generic_http.rs` | ~57KB | Generic HTTP Harness |
| `app/src/ai/agent_sdk/driver/harness/claude_code.rs` | ~24KB | Claude Code Harness |
| `app/src/ai/agent/api/local_llm_output.rs` | ~15KB | Local LLM 输出 |

## 附录 B: 编译单元统计

```
$ find app/src/ai -name "*.rs" | wc -l
391 files

$ find app/src/ai/blocklist -name "*.rs" | wc -l
148 files

$ wc -l app/src/ai/**/*.rs | tail -5
  [Total: ~150,000 lines across AI module]
```

---

*本文档基于 Warp 代码库分析生成*
*最后更新: 2026-05-14*
