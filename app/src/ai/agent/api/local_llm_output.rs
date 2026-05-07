//! Local LLM output generation for Warp GUI.
//!
//! This module provides local LLM support for the Warp GUI by generating
//! compatible ResponseStream items that can be processed by the existing
//! BlocklistAIController downstream.

use anyhow::anyhow;
use futures_util::{StreamExt, TryStreamExt};
use warp_multi_agent_api::{response_event, ClientAction, ResponseEvent};

use crate::ai::agent_sdk::driver::harness::generic_http::GenericProviderConfig;

use super::{ConvertToAPITypeError, Event, ResponseStream};

/// Load local LLM config if configured.
#[cfg(not(target_family = "wasm"))]
pub fn load_local_llm_config() -> Option<GenericProviderConfig> {
    GenericProviderConfig::load().ok().filter(|c| c.is_configured())
}

#[cfg(target_family = "wasm")]
pub fn load_local_llm_config() -> Option<GenericProviderConfig> {
    None
}

/// OpenAI-compatible chat request for local LLM.
#[derive(Debug, serde::Serialize)]
struct LocalChatRequest {
    model: String,
    messages: Vec<serde_json::Value>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<serde_json::Value>,
}

/// Extract user query from RequestParams.
fn extract_user_query(params: &super::RequestParams) -> String {
    for input in &params.input {
        if let super::AIAgentInput::UserQuery { query, .. } = input {
            return query.clone();
        }
    }
    String::new()
}

/// Build messages array from user query and context.
fn build_messages(user_query: &str, _context: &[super::AIAgentInput]) -> Vec<serde_json::Value> {
    let system_prompt = "You are Warp Terminal AI assistant. Help users with terminal commands, code explanations, and general questions. Keep responses concise and focused.";

    let mut messages = vec![
        serde_json::json!({
            "role": "system",
            "content": system_prompt
        }),
    ];

    if !user_query.is_empty() {
        messages.push(serde_json::json!({
            "role": "user",
            "content": user_query
        }));
    }

    messages
}

/// Generate local LLM output stream.
///
/// This function sends the request to a local LLM provider (Ollama, LM Studio, etc.)
/// and converts the SSE response to warp_multi_agent_api::ResponseEvent items.
#[cfg(not(target_family = "wasm"))]
pub async fn generate_local_llm_output(
    config: GenericProviderConfig,
    params: super::RequestParams,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    let client = reqwest::Client::new();

    // Build URL for OpenAI-compatible endpoint
    let base_url = config.base_url.trim_end_matches('/');
    let url = format!("{}/v1/chat/completions", base_url);

    // Extract user query from params
    let user_query = extract_user_query(&params);

    // Build messages for the request
    let messages = build_messages(&user_query, &params.input);

    // Create OpenAI-compatible request
    let request = LocalChatRequest {
        model: config.default_model.clone(),
        messages,
        stream: true,
        tools: None,
    };

    // Prepare request builder
    let mut req_builder = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request);

    // Add auth header if API key is provided
    if let Some(ref api_key) = config.api_key {
        req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
    }

    // Send request
    let response = req_builder.send().await.map_err(|e| {
        ConvertToAPITypeError::Other(anyhow!("Failed to connect to local LLM: {}", e))
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ConvertToAPITypeError::Other(anyhow!(
            "Local LLM request failed with status {}: {}",
            status, body
        )));
    }

    // Track conversation state
    let conversation_id = uuid::Uuid::new_v4().to_string();
    let request_id = uuid::Uuid::new_v4().to_string();
    let run_id = uuid::Uuid::new_v4().to_string();

    // Create channel for events (using async_channel which is in the project)
    let (tx, rx) = async_channel::unbounded::<Event>();

    // Spawn stream processing
    tokio::spawn(async move {
        // First, send init event
        let init_event = ResponseEvent {
            r#type: Some(response_event::Type::Init(response_event::StreamInit {
                conversation_id: conversation_id.clone(),
                request_id,
                run_id,
            })),
        };
        let _ = tx.send(Ok(init_event)).await;

        // Process SSE stream
        let mut stream = response.bytes_stream();

        let mut text_buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    // Try to parse as SSE line
                    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" {
                                    // Send finished event
                                    let finished_event = ResponseEvent {
                                        r#type: Some(response_event::Type::Finished(
                                            response_event::StreamFinished {
                                                reason: Some(
                                                    response_event::stream_finished::Reason::Done(
                                                        response_event::stream_finished::Done {}
                                                    )
                                                ),
                                                ..Default::default()
                                            }
                                        )),
                                    };
                                    let _ = tx.send(Ok(finished_event)).await;
                                    return;
                                }

                                // Try to parse JSON
                                match serde_json::from_str::<OpenAIStreamingChunk>(data) {
                                    Ok(chunk) => {
                                        for choice in chunk.choices {
                                            if let Some(content) = choice.delta.content {
                                                // Accumulate text
                                                text_buffer.push_str(&content);
                                            }

                                            // Handle tool calls
                                            if let Some(tool_calls) = choice.delta.tool_calls {
                                                // First, send any accumulated text as a message update
                                                if !text_buffer.is_empty() {
                                                    let text_to_send = text_buffer.clone();
                                                    text_buffer.clear();
                                                    let _ = tx.send(Ok(create_agent_output_message(text_to_send, &conversation_id))).await;
                                                }

                                                for tool_call in tool_calls {
                                                    let _ = tx.send(Ok(create_tool_suggestion(
                                                        &tool_call.function.name,
                                                        &tool_call.function.arguments,
                                                    ))).await;
                                                }
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        // Not JSON, treat as raw text
                                        text_buffer.push_str(&text);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let error_event = ResponseEvent {
                        r#type: Some(response_event::Type::Finished(
                            response_event::StreamFinished {
                                reason: Some(
                                    response_event::stream_finished::Reason::InternalError(
                                        response_event::stream_finished::InternalError {
                                            message: format!("Stream error: {}", e),
                                        }
                                    )
                                ),
                                ..Default::default()
                            }
                        )),
                    };
                    let _ = tx.send(Ok(error_event)).await;
                    return;
                }
            }
        }

        // If we have buffered text, send it as a message append
        if !text_buffer.is_empty() {
            let _ = tx.send(Ok(create_agent_output_message(text_buffer, &conversation_id))).await;
        }

        // Send finished event
        let finished_event = ResponseEvent {
            r#type: Some(response_event::Type::Finished(
                response_event::StreamFinished {
                    reason: Some(
                        response_event::stream_finished::Reason::Done(
                            response_event::stream_finished::Done {}
                        )
                    ),
                    ..Default::default()
                }
            )),
        };
        let _ = tx.send(Ok(finished_event)).await;
    });

    // Create stream from the async_channel receiver
    let stream = rx.take_until(cancellation_rx);

    Ok(Box::pin(stream))
}

#[cfg(target_family = "wasm")]
pub async fn generate_local_llm_output(
    _config: GenericProviderConfig,
    _params: super::RequestParams,
    _cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    Err(ConvertToAPITypeError::Other(anyhow!(
        "Local LLM not supported in WASM"
    )))
}

/// OpenAI streaming chunk format.
#[derive(Debug, serde::Deserialize)]
struct OpenAIStreamingChunk {
    choices: Vec<OpenAIStreamingChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAIStreamingChoice {
    delta: OpenAIDelta,
    #[serde(rename = "finish_reason")]
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAIDelta {
    content: Option<String>,
    #[serde(rename = "tool_calls")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: Option<String>,
    function: OpenAIFunction,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAIFunction {
    name: String,
    arguments: String,
}

/// Create a message action with agent output text.
fn create_agent_output_message(text: String, conversation_id: &str) -> ResponseEvent {
    use warp_multi_agent_api::client_action::Action;

    ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            response_event::ClientActions {
                actions: vec![
                    ClientAction {
                        action: Some(Action::AppendToMessageContent(
                            warp_multi_agent_api::client_action::AppendToMessageContent {
                                task_id: conversation_id.to_string(),
                                message: Some(warp_multi_agent_api::Message {
                                    id: format!("local-{}", uuid::Uuid::new_v4()),
                                    task_id: String::new(),
                                    request_id: String::new(),
                                    timestamp: None,
                                    server_message_data: String::new(),
                                    citations: vec![],
                                    message: Some(warp_multi_agent_api::message::Message::AgentOutput(
                                        warp_multi_agent_api::message::AgentOutput {
                                            text,
                                        }
                                    )),
                                }),
                                mask: None,
                            }
                        )),
                    }
                ],
            }
        )),
    }
}

/// Create a message action for tool calls.
fn create_tool_suggestion(name: &str, args: &str) -> ResponseEvent {
    use warp_multi_agent_api::client_action::Action;

    // For local LLM tool calls, we show them as agent output with tool info
    let suggestion_text = format!("# Tool: {} - {}\n\nNote: Tool execution requires server context.", name, args);

    ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            response_event::ClientActions {
                actions: vec![
                    ClientAction {
                        action: Some(Action::AppendToMessageContent(
                            warp_multi_agent_api::client_action::AppendToMessageContent {
                                task_id: String::new(),
                                message: Some(warp_multi_agent_api::Message {
                                    id: format!("local-tool-{}", uuid::Uuid::new_v4()),
                                    task_id: String::new(),
                                    request_id: String::new(),
                                    timestamp: None,
                                    server_message_data: String::new(),
                                    citations: vec![],
                                    message: Some(warp_multi_agent_api::message::Message::AgentOutput(
                                        warp_multi_agent_api::message::AgentOutput {
                                            text: suggestion_text,
                                        }
                                    )),
                                }),
                                mask: None,
                            }
                        )),
                    }
                ],
            }
        )),
    }
}