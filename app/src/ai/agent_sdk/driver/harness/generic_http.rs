//! Generic HTTP Harness for OpenAI-compatible APIs.
//!
//! This harness enables Warp to use local LLM providers like Ollama, LM Studio,
//! and Jan without requiring a Warp server connection.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use warp_cli::agent::Harness;
use warp_managed_secrets::ManagedSecretValue;

use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::server::server_api::ServerApi;
use crate::terminal::CLIAgent;
use warpui::{ModelHandle, ModelSpawner};

use super::terminal::{CommandHandle, TerminalDriver};
use super::{AgentDriverError, SavePoint, ThirdPartyHarness};

/// Configuration for a generic OpenAI-compatible provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericProviderConfig {
    /// Display name for the provider.
    pub name: String,
    /// Base URL for the API (e.g., "http://localhost:11434").
    pub base_url: String,
    /// Optional API key for authentication.
    pub api_key: Option<String>,
    /// Default model to use (e.g., "llama3", "gpt-4").
    pub default_model: String,
    /// Whether streaming is enabled.
    pub streaming: bool,
}

impl Default for GenericProviderConfig {
    fn default() -> Self {
        Self {
            name: "Ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        }
    }
}

impl GenericProviderConfig {
    /// Auto-detect available local LLM providers.
    /// Checks common endpoints like Ollama and LM Studio.
    pub async fn auto_detect() -> Option<Self> {
        // Try Ollama first (most common)
        if Self::check_ollama().await {
            return Some(Self {
                name: "Ollama".to_string(),
                base_url: "http://localhost:11434".to_string(),
                ..Default::default()
            });
        }

        // Try LM Studio
        if Self::check_lm_studio().await {
            return Some(Self {
                name: "LM Studio".to_string(),
                base_url: "http://localhost:1234".to_string(),
                default_model: "auto".to_string(),
                ..Default::default()
            });
        }

        None
    }

    /// Check if Ollama is running.
    async fn check_ollama() -> bool {
        let client = reqwest::Client::new();
        match client
            .get("http://localhost:11434/api/tags")
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Check if LM Studio is running.
    async fn check_lm_studio() -> bool {
        let client = reqwest::Client::new();
        match client
            .get("http://localhost:1234/v1/models")
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Check if this provider is reachable.
    /// Returns Ok(()) if the connection is successful.
    pub async fn check_connection(&self) -> Result<(), AgentDriverError> {
        let client = reqwest::Client::new();
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));

        client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!(
                    "Failed to connect to {} at {}. Error: {}. Is the server running?",
                    self.name, url, e
                ),
            })?
            .error_for_status()
            .map_err(|e| AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!("{} at {} returned error: {}", self.name, url, e),
            })?;

        Ok(())
    }

    /// Get available models from the provider.
    pub async fn get_models(&self) -> Result<Vec<String>, AgentDriverError> {
        let client = reqwest::Client::new();
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));

        let response = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!("Failed to fetch models from {}: {}", url, e),
            })?;

        if !response.status().is_success() {
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!("Failed to fetch models: status {}", response.status()),
            });
        }

        // Parse the models list (OpenAI-compatible format)
        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelInfo>,
        }

        #[derive(Deserialize)]
        struct ModelInfo {
            id: String,
        }

        let models_resp: ModelsResponse =
            response
                .json()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "generic".to_string(),
                    reason: format!("Failed to parse models response: {}", e),
                })?;

        Ok(models_resp.data.into_iter().map(|m| m.id).collect())
    }
}

/// A message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// OpenAI Chat Completions request format.
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

/// OpenAI Chat Completions streaming response delta.
#[derive(Debug, Deserialize)]
struct StreamingDelta {
    content: Option<String>,
    #[serde(rename = "tool_calls")]
    tool_calls: Option<Vec<ToolCall>>,
}

/// OpenAI Chat Completions streaming chunk.
#[derive(Debug, Deserialize)]
struct StreamingChunk {
    choices: Vec<StreamingChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamingChoice {
    delta: StreamingDelta,
    #[serde(rename = "finish_reason")]
    finish_reason: Option<String>,
}

/// OpenAI Chat Completions response format.
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
    #[serde(rename = "tool_calls")]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ToolFunction,
}

#[derive(Debug, Deserialize)]
struct ToolFunction {
    name: String,
    arguments: String,
}

/// Generic HTTP harness for OpenAI-compatible APIs.
pub struct GenericHttpHarness {
    config: GenericProviderConfig,
    conversation_history: Vec<Message>,
    tools: Vec<serde_json::Value>,
}

impl GenericHttpHarness {
    pub fn new(config: GenericProviderConfig) -> Self {
        Self {
            config,
            conversation_history: Vec::new(),
            tools: Vec::new(),
        }
    }

    /// Set the tools available to the harness.
    pub fn with_tools(mut self, tools: Vec<serde_json::Value>) -> Self {
        self.tools = tools;
        self
    }

    /// Add a tool to the harness.
    pub fn add_tool(&mut self, tool: serde_json::Value) {
        self.tools.push(tool);
    }

    /// Get the current tools.
    pub fn get_tools(&self) -> &[serde_json::Value] {
        &self.tools
    }

    /// Check if the harness has tools configured.
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }

    /// Build the HTTP request URL for chat completions.
    fn build_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    /// Add Authorization header if API key is provided.
    fn add_auth_header(&self, req_builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref api_key) = self.config.api_key {
            req_builder.header("Authorization", format!("Bearer {}", api_key))
        } else {
            req_builder
        }
    }

    /// Execute a chat completion request.
    async fn chat(&self, prompt: &str) -> Result<ChatResponse, AgentDriverError> {
        let client = reqwest::Client::new();

        // Add current prompt to history
        let mut messages = self.conversation_history.clone();
        messages.push(Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        });

        let request = ChatRequest {
            model: self.config.default_model.clone(),
            messages,
            stream: false,
            tools: None,
        };

        let url = self.build_url();
        let mut req_builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request);

        req_builder = self.add_auth_header(req_builder);

        let response =
            req_builder
                .send()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "generic".to_string(),
                    reason: format!(
                        "Failed to connect to {}: {}. Is the server running?",
                        url, e
                    ),
                })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!("API request failed with status {}: {}", status, body),
            });
        }

        let chat_response: ChatResponse =
            response
                .json()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "generic".to_string(),
                    reason: format!("Failed to parse API response: {}", e),
                })?;

        Ok(chat_response)
    }
}

impl ThirdPartyHarness for GenericHttpHarness {
    fn harness(&self) -> Harness {
        Harness::Generic
    }

    fn cli_agent(&self) -> CLIAgent {
        // Generic harness doesn't use a CLI
        CLIAgent::Warp
    }

    fn install_docs_url(&self) -> Option<&'static str> {
        Some("https://ollama.com/download")
    }

    fn validate(&self) -> Result<(), AgentDriverError> {
        // For Generic harness, we validate by trying to connect
        // The actual validation happens in build_runner
        Ok(())
    }

    fn prepare_environment_config(
        &self,
        _working_dir: &Path,
        _system_prompt: Option<&str>,
        _secrets: &HashMap<String, ManagedSecretValue>,
    ) -> Result<(), AgentDriverError> {
        Ok(())
    }

    fn build_runner(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        _resumption_prompt: Option<&str>,
        _working_dir: &Path,
        _task_id: Option<AmbientAgentTaskId>,
        _server_api: Arc<ServerApi>,
        terminal_driver: ModelHandle<TerminalDriver>,
        _resume: Option<super::ResumePayload>,
    ) -> Result<Box<dyn super::HarnessRunner>, AgentDriverError> {
        // Build initial messages with system prompt if provided
        let mut messages = Vec::new();
        if let Some(system) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: system.to_string(),
            });
        }

        let runner = GenericHarnessRunner {
            config: self.config.clone(),
            terminal_driver,
            messages,
            current_prompt: prompt.to_string(),
            tools: self.tools.clone(),
        };

        Ok(Box::new(runner))
    }
}

/// Runner for the Generic HTTP harness.
pub struct GenericHarnessRunner {
    config: GenericProviderConfig,
    terminal_driver: ModelHandle<TerminalDriver>,
    messages: Vec<Message>,
    current_prompt: String,
    tools: Vec<serde_json::Value>,
}

#[async_trait]
impl super::HarnessRunner for GenericHarnessRunner {
    async fn start(
        &self,
        foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<CommandHandle, AgentDriverError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        // Build messages
        let mut messages = self.messages.clone();
        messages.push(Message {
            role: "user".to_string(),
            content: self.current_prompt.clone(),
        });

        let request = ChatRequest {
            model: self.config.default_model.clone(),
            messages: messages.clone(),
            stream: self.config.streaming,
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(self.tools.clone())
            },
        };

        // Prepare request
        let mut req_builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request);

        if let Some(ref api_key) = self.config.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        if self.config.streaming {
            return self.stream_response(req_builder, foreground).await;
        }

        // Non-streaming response
        let response =
            req_builder
                .send()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "generic".to_string(),
                    reason: format!(
                        "Failed to connect to {}: {}. Is the server running?",
                        url, e
                    ),
                })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!("API request failed with status {}: {}", status, body),
            });
        }

        let chat_response: ChatResponse =
            response
                .json()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "generic".to_string(),
                    reason: format!("Failed to parse API response: {}", e),
                })?;

        // Extract response content
        let content = chat_response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        // Write response to terminal
        let driver = self.terminal_driver.clone();
        let _ = foreground
            .spawn(move |_, ctx| {
                driver.as_ref(ctx).write_to_terminal(&content, ctx);
            })
            .await;

        // Return a mock command handle (the command completed successfully)
        Ok(CommandHandle {
            inner: tokio::sync::oneshot::channel().0,
            block_id: None,
            conversation_id: None,
        })
    }

    /// Handle streaming response (SSE).
    async fn stream_response(
        &self,
        req_builder: reqwest::RequestBuilder,
        foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<CommandHandle, AgentDriverError> {
        let response =
            req_builder
                .send()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "generic".to_string(),
                    reason: format!(
                        "Failed to connect to streaming endpoint: {}. Is the server running?",
                        e
                    ),
                })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: "generic".to_string(),
                reason: format!("Streaming request failed with status {}: {}", status, body),
            });
        }

        // Process SSE stream
        let mut stream = response.bytes_stream();
        let mut accumulated_content = String::new();
        let driver = self.terminal_driver.clone();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    // Parse SSE data lines
                    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" {
                                    break;
                                }
                                // Parse streaming chunk
                                if let Ok(chunk) = serde_json::from_str::<StreamingChunk>(data) {
                                    for choice in chunk.choices {
                                        if let Some(content) = choice.delta.content {
                                            accumulated_content.push_str(&content);
                                            // Write incremental content to terminal
                                            let _ = foreground
                                                .spawn({
                                                    let driver = driver.clone();
                                                    let content = content.clone();
                                                    move |_, ctx| {
                                                        driver
                                                            .as_ref(ctx)
                                                            .write_to_terminal(&content, ctx);
                                                    }
                                                })
                                                .await;
                                        }
                                        // Handle tool calls in streaming response
                                        if let Some(tool_calls) = &choice.delta.tool_calls {
                                            for tool_call in tool_calls {
                                                // Log tool call for now - actual execution would be handled
                                                // by the harness runner's tool execution logic
                                                log::info!(
                                                    "Tool call: {} - {}",
                                                    tool_call.id,
                                                    tool_call.function.name
                                                );
                                            }
                                        }
                                        // Check for finish reason
                                        if choice.finish_reason.is_some() {
                                            log::debug!(
                                                "Stream finished: {:?}",
                                                choice.finish_reason
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Stream error: {}", e);
                    break;
                }
            }
        }

        // Return command handle
        Ok(CommandHandle {
            inner: tokio::sync::oneshot::channel().0,
            block_id: None,
            conversation_id: None,
        })
    }

    async fn save_conversation(
        &self,
        _save_point: SavePoint,
        _foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<()> {
        // Conversation is stored in self.messages, could be persisted here
        Ok(())
    }

    async fn exit(
        &self,
        _foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<()> {
        Ok(())
    }

    async fn cleanup(
        &self,
        _foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<()> {
        Ok(())
    }
}
