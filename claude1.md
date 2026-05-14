# Warp 本地 LLM 集成修复计划

> **版本**: v1.0
> **日期**: 2026-05-14
> **目标**: 修复本地 LLM 响应在 GUI 中不显示的问题

---

## 问题分析

### 症状

运行 Warp 并使用本地 LLM (MiniMax) 时：
1. API 调用成功，返回 Anthropic SSE 格式响应
2. 日志显示 `Received unsupported client action: AppendToMessageContent`
3. AI 响应不显示在 GUI 中
4. 错误：`Failed to get access token for GraphQL request: Attempted to retrieve access token when user is logged out`

### 根因分析

#### 1. 事件流协议不匹配

Warp GUI 期望的事件流协议：

```
Init (request_id) → CreateTask (创建任务和Exchange) → AppendToMessageContent (更新消息)
```

本地 LLM 当前实现 (`local_llm_output.rs`)：

```
Init → AppendToMessageContent (直接发送，没有创建任务/Exchange)
```

#### 2. 关键代码位置

**conversation.rs:2474-2488** - AppendToMessageContent 处理需要:
```rust
Action::AppendToMessageContent(AppendToMessageContent {
    task_id,
    message: Some(message),
    mask: Some(mask),  // 必须有 mask!
}) => {
    // 需要从 added_exchanges_by_response 查找 exchange
    let exchange_id = self
        .added_exchanges_by_response
        .get(response_stream_id)  // 必须有这个 entry!
        .ok_or(UpdateConversationError::NoPendingRequest)?
        ...
}
```

**关键问题**: `added_exchanges_by_response` 没有为本地 LLM 响应设置条目，因为没有先发送 `CreateTask`。

#### 3. 事件类型解析

MiniMax 返回的 Anthropic SSE 格式：
```
event: message_start
event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"我是 Warp Terminal AI 助手..."}}
event: message_stop
```

需要正确解析这些事件并转换为我们的事件格式。

---

## 实现方案

### 方案 A: 实现完整的 CreateTask 流程 (推荐)

修改 `local_llm_output.rs`，在发送 `AppendToMessageContent` 之前先发送 `CreateTask`:

#### 1. 添加 CreateTask 事件创建函数

```rust
/// 创建 CreateTask 事件以初始化任务和 Exchange
fn create_task_event(
    task_id: &str,
    conversation_id: &str,
) -> ResponseEvent {
    use warp_multi_agent_api::client_action::Action;
    use warp_multi_agent_api::response_event::{self, ClientActions, ResponseEvent, StreamInit};
    use warp_multi_agent_api::{Task, TaskSource};

    ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            ClientActions {
                actions: vec![
                    ClientAction {
                        action: Some(Action::CreateTask(
                            warp_multi_agent_api::client_action::CreateTask {
                                task: Some(Task {
                                    id: task_id.to_string(),
                                    task_source: Some(TaskSource {
                                        subagent_params: None,
                                        ..Default::default()
                                    }),
                                    parent_id: None,
                                    ..Default::default()
                                }),
                            }
                        )),
                    }
                ],
            }
        )),
    }
}
```

#### 2. 修改 Init 事件包含 request_id

```rust
let init_event = ResponseEvent {
    r#type: Some(response_event::Type::Init(StreamInit {
        request_id: request_id.clone(),
        conversation_id: conversation_id.clone(),
        run_id: run_id.clone(),
    })),
};
```

#### 3. 确保 mask 不为 None

当前 `create_agent_output_message` 设置 `mask: None`，需要提供有效的 mask。

### 方案 B: 使用简化的本地模式 (备选)

创建一个独立的本地 LLM 渲染路径，不经过完整的 BlocklistAICController，直接更新 UI。

---

## 实现步骤

### Phase 1: 修复 Anthropic SSE 解析 (已完成)

**文件**: `app/src/ai/agent/api/local_llm_output.rs`

**修改**:
- 添加 `AnthropicSSEChunk` 结构体
- 解析 `event:` 前缀识别事件类型
- 正确处理 `content_block_delta` 事件中的文本

### Phase 2: 实现 CreateTask 流程 (待完成)

**文件**: `app/src/ai/agent/api/local_llm_output.rs`

**修改**:
1. 在 `Init` 之后立即发送 `CreateTask` 事件
2. 创建任务并设置 `added_exchanges_by_response` 条目
3. 使用正确的 `task_id` 发送 `AppendToMessageContent`

### Phase 3: 修复 mask 问题 (待完成)

**分析**: `AppendToMessageContent` 需要 `mask: Some(mask)`，当前传 `None`

**可能解决方案**:
- 使用默认 mask
- 查阅 BlocklistAIController 如何获取 mask

### Phase 4: 测试验证 (待完成)

1. 编译 `cargo build --bin warp-oss`
2. 运行 Warp 并配置 MiniMax
3. 验证响应显示在 GUI 中

---

## 相关文件

| 文件 | 作用 |
|------|------|
| `app/src/ai/agent/api/local_llm_output.rs` | 本地 LLM 输出生成 |
| `app/src/ai/agent/conversation.rs` | Conversation 状态管理 |
| `app/src/ai/blocklist/controller/response_stream.rs` | 响应流处理 |
| `app/src/ai/agent_sdk/driver/harness/generic_http.rs` | LLM Provider 配置 |

---

## API 协议分析

### Warp Multi-Agent API 事件流

基于 `warp_multi_agent_api::response_event::Type`:

1. **Init** - 初始化流
   ```rust
   Type::Init(StreamInit {
       conversation_id: String,
       request_id: String,
       run_id: String,
   })
   ```

2. **ClientActions** - 客户端操作
   ```rust
   Type::ClientActions(ClientActions {
       actions: Vec<ClientAction>,
   })
   ```

3. **Finished** - 流结束
   ```rust
   Type::Finished(StreamFinished {
       reason: Option<Reason::Done | Reason::InternalError | ...>,
   })
   ```

### ClientAction 类型

基于 `warp_multi_agent_api::client_action::Action`:

| Action | 用途 |
|--------|------|
| `CreateTask` | 创建新任务和关联的 Exchange |
| `AppendToMessageContent` | 向消息追加内容 |
| `ShowSuggestions` | 显示建议 |
| `MoveMessagesToNewTask` | 移动消息到新任务 |

### Local LLM 当前使用

当前 `local_llm_output.rs` 只使用:
- `Init` ✅
- `AppendToMessageContent` ❌ (缺少前置 CreateTask)

---

## 与 Claude Code 的关系

### 架构对比

| 组件 | Claude Code | Warp |
|------|-------------|------|
| Agent 核心 | Claude Agent SDK | warp_multi_agent_api |
| 任务管理 | Task/Subtask | Task/Exchange |
| 工具执行 | Native tools | Server-backed tools |
| 流协议 | Anthropic Event Stream | Custom ResponseEvent |

### 关键差异

1. **工具执行**: Claude Code 本地执行工具，Warp 需要 Warp Server
2. **认证**: Claude Code 使用 API key，Warp 需要 Warp Server 认证
3. **上下文**: Claude Code 管理完整上下文，Warp 通过 Conversation 管理

### Local LLM 实现借鉴

本地 LLM 实现 (`local_llm_output.rs`) 借鉴了 Claude Code 的思路:
- SSE 流解析
- 增量文本更新
- 响应事件转换

但 Warp 的 GUI 期望完整的多代理协议，不是简化的 Claude Code 风格。

---

## 待解决问题

1. [ ] `CreateTask` 事件的正确格式和字段
2. [ ] `mask` 参数的正确来源
3. [ ] `added_exchanges_by_response` 的初始化
4. [ ] 对话历史的本地持久化

---

## 参考文献

- `app/src/ai/agent/conversation.rs:2019` - CreateTask 处理
- `app/src/ai/agent/conversation.rs:2474` - AppendToMessageContent 处理
- `crates/warp-multi-agent-api/` - Multi-Agent API 定义
- MiniMax API 文档 - Anthropic Event Stream 格式
