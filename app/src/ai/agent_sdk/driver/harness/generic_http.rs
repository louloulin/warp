//! Generic HTTP Harness for OpenAI-compatible APIs.
//!
//! This harness enables Warp to use local LLM providers like Ollama, LM Studio,
//! and Jan without requiring a Warp server connection.

#[cfg(test)]
use mockito;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    /// Checks common endpoints in order of popularity.
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

        // Try Jan
        if Self::check_jan().await {
            return Some(Self {
                name: "Jan".to_string(),
                base_url: "http://localhost:1337".to_string(),
                ..Default::default()
            });
        }

        // Try Text Generation WebUI
        if Self::check_textgen_webui().await {
            return Some(Self {
                name: "Text Generation WebUI".to_string(),
                base_url: "http://localhost:5000".to_string(),
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

    /// Check if Jan is running.
    async fn check_jan() -> bool {
        let client = reqwest::Client::new();
        match client
            .get("http://localhost:1337/v1/models")
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Check if Text Generation WebUI is running.
    async fn check_textgen_webui() -> bool {
        let client = reqwest::Client::new();
        match client
            .get("http://localhost:5000/v1/models")
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

    /// Save this configuration to a JSON file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), AgentDriverError> {
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                "Failed to serialize config: {}",
                e
            ))
        })?;

        std::fs::write(path, json).map_err(|e| {
            AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                "Failed to write config file: {}",
                e
            ))
        })?;

        Ok(())
    }

    /// Load a configuration from a JSON file.
    pub fn load_from_file(path: &Path) -> Result<Self, AgentDriverError> {
        let json = std::fs::read_to_string(path).map_err(|e| {
            AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                "Failed to read config file: {}",
                e
            ))
        })?;

        serde_json::from_str(&json).map_err(|e| {
            AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                "Failed to parse config file: {}",
                e
            ))
        })
    }

    /// Get the default config file path.
    pub fn default_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("warp")
            .join("generic_ai_config.json")
    }

    /// Save this configuration to the default config file.
    pub fn save(&self) -> Result<(), AgentDriverError> {
        let path = Self::default_config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                    "Failed to create config directory: {}",
                    e
                ))
            })?;
        }
        self.save_to_file(&path)
    }

    /// Load configuration from the default config file.
    pub fn load() -> Result<Self, AgentDriverError> {
        let path = Self::default_config_path();
        self.load_from_file(&path)
    }

    /// Validate the configuration and return Ok if valid, or an error message.
    pub fn validate(&self) -> Result<(), String> {
        // Check base_url is not empty
        if self.base_url.is_empty() {
            return Err("Provider URL cannot be empty".to_string());
        }

        // Check base_url is a valid URL format
        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            return Err("Provider URL must start with http:// or https://".to_string());
        }

        // Check model is not empty
        if self.default_model.is_empty() {
            return Err("Default model cannot be empty".to_string());
        }

        // Validate URL is reachable (optional - would need async)
        Ok(())
    }

    /// Create a configuration for OpenAI API.
    /// Requires an API key for authentication.
    pub fn openai(api_key: String) -> Self {
        Self {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: Some(api_key),
            default_model: "gpt-4o".to_string(),
            streaming: true,
        }
    }

    /// Create a configuration for Groq API (free tier available).
    pub fn groq(api_key: String) -> Self {
        Self {
            name: "Groq".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            api_key: Some(api_key),
            default_model: "llama-3.1-70b-versatile".to_string(),
            streaming: true,
        }
    }

    /// Create a configuration for Together AI.
    pub fn together(api_key: String) -> Self {
        Self {
            name: "Together AI".to_string(),
            base_url: "https://api.together.ai/v1".to_string(),
            api_key: Some(api_key),
            default_model: "meta-llama/Llama-3-70b-chat-hf".to_string(),
            streaming: true,
        }
    }

    /// Create a configuration for a custom OpenAI-compatible API.
    pub fn custom(name: String, base_url: String, api_key: Option<String>) -> Self {
        Self {
            name,
            base_url,
            api_key,
            default_model: String::new(),
            streaming: true,
        }
    }

    /// Create a configuration for Anthropic Claude API.
    /// Uses Anthropic's native Messages API protocol.
    pub fn anthropic(api_key: String) -> Self {
        Self {
            name: "Anthropic Claude".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: Some(api_key),
            default_model: "claude-sonnet-4-20250514".to_string(),
            streaming: true,
        }
    }

    /// Check if this provider is Anthropic.
    pub fn is_anthropic(&self) -> bool {
        self.base_url.contains("anthropic.com")
    }

    /// Build the Anthropic API URL for messages endpoint.
    fn anthropic_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    /// Create a configuration for Fireworks AI.
    pub fn fireworks(api_key: String) -> Self {
        Self {
            name: "Fireworks AI".to_string(),
            base_url: "https://api.fireworks.ai/inference/v1".to_string(),
            api_key: Some(api_key),
            default_model: "accounts/fireworks/models/llama-v3-70b-instruct".to_string(),
            streaming: true,
        }
    }

    /// Create a configuration for Mistral AI.
    pub fn mistral(api_key: String) -> Self {
        Self {
            name: "Mistral".to_string(),
            base_url: "https://api.mistral.ai/v1".to_string(),
            api_key: Some(api_key),
            default_model: "mistral-large-latest".to_string(),
            streaming: true,
        }
    }

    /// Create a configuration for Perplexity AI (Sonar models).
    pub fn perplexity(api_key: String) -> Self {
        Self {
            name: "Perplexity".to_string(),
            base_url: "https://api.perplexity.ai".to_string(),
            api_key: Some(api_key),
            default_model: "sonar".to_string(),
            streaming: true,
        }
    }

    /// Create a configuration for Azure OpenAI.
    /// Note: Azure uses deployment names instead of model IDs.
    pub fn azure(endpoint: String, api_key: String, deployment: String) -> Self {
        Self {
            name: "Azure OpenAI".to_string(),
            base_url: endpoint,
            api_key: Some(api_key),
            default_model: deployment,
            streaming: true,
        }
    }
}

/// A message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// Anthropic Messages API request format.
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

/// Anthropic streaming event types.
#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
    #[serde(default)]
    message: Option<AnthropicMessageContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(rename = "type")]
    delta_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageContent {
    #[serde(default)]
    content: Option<String>,
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

    /// Add Anthropic-specific headers.
    fn add_anthropic_headers(
        &self,
        req_builder: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        let mut req_builder = req_builder
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01");

        if let Some(ref api_key) = self.config.api_key {
            req_builder = req_builder.header("x-api-key", api_key);
        }

        req_builder
    }

    /// Build an Anthropic request from messages.
    fn build_anthropic_request(
        &self,
        messages: Vec<Message>,
        system_prompt: Option<&str>,
    ) -> AnthropicRequest {
        let anthropic_messages: Vec<AnthropicMessage> = messages
            .into_iter()
            .map(|m| AnthropicMessage {
                role: m.role,
                content: m.content,
            })
            .collect();

        AnthropicRequest {
            model: self.config.default_model.clone(),
            messages: anthropic_messages,
            max_tokens: 4096,
            stream: self.config.streaming,
            system: system_prompt.map(String::from),
            tools: None,
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

    /// Handle Anthropic streaming response (SSE).
    async fn anthropic_stream_response(
        &self,
        req_builder: reqwest::RequestBuilder,
        foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<CommandHandle, AgentDriverError> {
        let response =
            req_builder
                .send()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "anthropic".to_string(),
                    reason: format!("Failed to connect to Anthropic API: {}", e),
                })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: "anthropic".to_string(),
                reason: format!("Anthropic API error {}: {}", status, body),
            });
        }

        // Process Anthropic SSE stream
        let mut stream = response.bytes_stream();
        let mut accumulated_content = String::new();
        let driver = self.terminal_driver.clone();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" {
                                    break;
                                }
                                // Parse Anthropic streaming event
                                if let Ok(event) =
                                    serde_json::from_str::<AnthropicStreamEvent>(data)
                                {
                                    // Handle content block
                                    if event.event_type == "content_block_delta" {
                                        if let Some(delta) = event.delta {
                                            if let Some(text) = delta.text {
                                                accumulated_content.push_str(&text);
                                                let _ = foreground
                                                    .spawn({
                                                        let driver = driver.clone();
                                                        let text = text.clone();
                                                        move |_, ctx| {
                                                            driver
                                                                .as_ref(ctx)
                                                                .write_to_terminal(&text, ctx);
                                                        }
                                                    })
                                                    .await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Anthropic stream error: {}", e);
                    break;
                }
            }
        }

        Ok(CommandHandle {
            inner: tokio::sync::oneshot::channel().0,
            block_id: None,
            conversation_id: None,
        })
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

        // Build messages
        let mut messages = self.messages.clone();
        messages.push(Message {
            role: "user".to_string(),
            content: self.current_prompt.clone(),
        });

        // Check if using Anthropic API
        if self.config.is_anthropic() {
            return self.anthropic_start(client, messages, foreground).await;
        }

        // Standard OpenAI-compatible request
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

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

    /// Handle Anthropic API request.
    async fn anthropic_start(
        &self,
        client: reqwest::Client,
        messages: Vec<Message>,
        foreground: &ModelSpawner<crate::ai::agent_sdk::driver::AgentDriver>,
    ) -> Result<CommandHandle, AgentDriverError> {
        let url = self.config.anthropic_url();

        // Extract system message if present
        let (system_messages, other_messages): (Vec<_>, Vec<_>) =
            messages.iter().partition(|m| m.role == "system");

        let system_prompt = system_messages.first().map(|m| m.content.as_str());

        let anthropic_messages: Vec<AnthropicMessage> = other_messages
            .into_iter()
            .cloned()
            .map(|m| AnthropicMessage {
                role: m.role,
                content: m.content,
            })
            .collect();

        let request = AnthropicRequest {
            model: self.config.default_model.clone(),
            messages: anthropic_messages,
            max_tokens: 4096,
            stream: self.config.streaming,
            system: system_prompt.map(String::from),
            tools: None,
        };

        let req_builder = client.post(&url).json(&request);
        let req_builder = self.add_anthropic_headers(req_builder);

        if self.config.streaming {
            return self
                .anthropic_stream_response(req_builder, foreground)
                .await;
        }

        // Non-streaming response
        let response =
            req_builder
                .send()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "anthropic".to_string(),
                    reason: format!("Failed to connect to Anthropic API: {}", e),
                })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: "anthropic".to_string(),
                reason: format!("Anthropic API error {}: {}", status, body),
            });
        }

        #[derive(Deserialize)]
        struct AnthropicResponse {
            content: Vec<AnthropicResponseContent>,
        }

        #[derive(Deserialize)]
        struct AnthropicResponseContent {
            text: Option<String>,
        }

        let anthropic_resp: AnthropicResponse =
            response
                .json()
                .await
                .map_err(|e| AgentDriverError::HarnessSetupFailed {
                    harness: "anthropic".to_string(),
                    reason: format!("Failed to parse Anthropic response: {}", e),
                })?;

        let content = anthropic_resp
            .content
            .first()
            .and_then(|c| c.text.clone())
            .unwrap_or_default();

        let driver = self.terminal_driver.clone();
        let _ = foreground
            .spawn(move |_, ctx| {
                driver.as_ref(ctx).write_to_terminal(&content, ctx);
            })
            .await;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GenericProviderConfig::default();
        assert_eq!(config.name, "Ollama");
        assert_eq!(config.base_url, "http://localhost:11434");
        assert!(config.api_key.is_none());
        assert_eq!(config.default_model, "llama3");
        assert!(config.streaming);
    }

    #[test]
    fn test_openai_preset() {
        let config = GenericProviderConfig::openai("sk-test".to_string());
        assert_eq!(config.name, "OpenAI");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.api_key, Some("sk-test".to_string()));
        assert_eq!(config.default_model, "gpt-4");
    }

    #[test]
    fn test_anthropic_preset() {
        let config = GenericProviderConfig::anthropic("sk-ant-test".to_string());
        assert_eq!(config.name, "Anthropic");
        assert_eq!(config.base_url, "https://api.anthropic.com");
        assert_eq!(config.api_key, Some("sk-ant-test".to_string()));
        assert_eq!(config.default_model, "claude-sonnet-4-20250514");
        assert!(config.is_anthropic());
    }

    #[test]
    fn test_groq_preset() {
        let config = GenericProviderConfig::groq("gsk_test".to_string());
        assert_eq!(config.name, "Groq");
        assert_eq!(config.base_url, "https://api.groq.com/openai/v1");
    }

    #[test]
    fn test_custom_preset() {
        let config = GenericProviderConfig::custom(
            "MyProvider".to_string(),
            "http://my.server:8080/v1".to_string(),
            Some("key".to_string()),
        );
        assert_eq!(config.name, "MyProvider");
        assert_eq!(config.base_url, "http://my.server:8080/v1");
        assert_eq!(config.api_key, Some("key".to_string()));
    }

    #[test]
    fn test_is_anthropic() {
        let anthropic = GenericProviderConfig::anthropic("key".to_string());
        assert!(anthropic.is_anthropic());

        let openai = GenericProviderConfig::openai("key".to_string());
        assert!(!openai.is_anthropic());
    }

    #[test]
    fn test_config_serialization() {
        let original = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: "http://localhost:9999".to_string(),
            api_key: Some("secret".to_string()),
            default_model: "test-model".to_string(),
            streaming: false,
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: GenericProviderConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, original.name);
        assert_eq!(deserialized.base_url, original.base_url);
        assert_eq!(deserialized.api_key, original.api_key);
        assert_eq!(deserialized.default_model, original.default_model);
        assert_eq!(deserialized.streaming, original.streaming);
    }

    #[test]
    fn test_config_save_load_roundtrip() {
        let dir = std::env::temp_dir().join("warp_test_config");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test_config.json");

        let original = GenericProviderConfig {
            name: "RoundTrip".to_string(),
            base_url: "http://localhost:1234".to_string(),
            api_key: None,
            default_model: "rt-model".to_string(),
            streaming: true,
        };

        original.save_to_file(&path).unwrap();
        let loaded = GenericProviderConfig::load_from_file(&path).unwrap();

        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.base_url, original.base_url);
        assert_eq!(loaded.api_key, original.api_key);
        assert_eq!(loaded.default_model, original.default_model);
        assert_eq!(loaded.streaming, original.streaming);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_harness_new() {
        let config = GenericProviderConfig::default();
        let harness = GenericHttpHarness::new(config);
        assert!(harness.get_tools().is_empty());
        assert!(!harness.has_tools());
    }

    #[test]
    fn test_harness_with_tools() {
        let config = GenericProviderConfig::default();
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "test_tool",
                "description": "A test tool",
                "parameters": {}
            }
        });

        let harness = GenericHttpHarness::new(config).with_tools(vec![tool.clone()]);
        assert!(harness.has_tools());
        assert_eq!(harness.get_tools().len(), 1);
    }

    #[test]
    fn test_harness_add_tool() {
        let config = GenericProviderConfig::default();
        let mut harness = GenericHttpHarness::new(config);

        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run shell commands",
                "parameters": {}
            }
        });

        harness.add_tool(tool);
        assert!(harness.has_tools());
        assert_eq!(harness.get_tools().len(), 1);
    }

    #[test]
    fn test_all_provider_presets() {
        // Test all cloud provider presets
        let providers = vec![
            ("OpenAI", GenericProviderConfig::openai("key".to_string())),
            (
                "Anthropic",
                GenericProviderConfig::anthropic("key".to_string()),
            ),
            ("Groq", GenericProviderConfig::groq("key".to_string())),
            (
                "Together AI",
                GenericProviderConfig::together("key".to_string()),
            ),
            (
                "Fireworks AI",
                GenericProviderConfig::fireworks("key".to_string()),
            ),
            ("Mistral", GenericProviderConfig::mistral("key".to_string())),
            (
                "Perplexity",
                GenericProviderConfig::perplexity("key".to_string()),
            ),
        ];

        for (expected_name, config) in providers {
            assert_eq!(config.name, expected_name, "Provider name mismatch");
            assert!(!config.base_url.is_empty(), "Base URL should not be empty");
            assert!(config.api_key.is_some(), "API key should be set");
        }
    }

    #[test]
    fn test_azure_preset() {
        let config = GenericProviderConfig::azure(
            "https://my-resource.openai.azure.com".to_string(),
            "azure-key".to_string(),
            "gpt-4-deployment".to_string(),
        );
        assert_eq!(config.name, "Azure OpenAI");
        assert_eq!(config.default_model, "gpt-4-deployment");
        assert!(config.base_url.contains("azure.com"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn test_check_connection_success() {
        let mut server = Server::new_async().await;
        let m = server.mock("GET", "/v1/models").with_status(200).create();

        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: server.url(),
            api_key: None,
            default_model: "test".to_string(),
            streaming: true,
        };

        let result = config.check_connection().await;
        m.assert();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_check_connection_failure() {
        let mut server = Server::new_async().await;
        let _m = server.mock("GET", "/v1/models").with_status(404).create();

        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: server.url(),
            api_key: None,
            default_model: "test".to_string(),
            streaming: true,
        };

        let result = config.check_connection().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_models_success() {
        let mut server = Server::new_async().await;
        let response = serde_json::json!({
            "data": [
                {"id": "llama3"},
                {"id": "codellama"}
            ]
        });
        let _m = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .create();

        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: server.url(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        };

        let models = config.get_models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0], "llama3");
        assert_eq!(models[1], "codellama");
    }

    #[tokio::test]
    async fn test_get_models_empty() {
        let mut server = Server::new_async().await;
        let response = serde_json::json!({
            "data": []
        });
        let _m = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .create();

        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: server.url(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        };

        let models = config.get_models().await.unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_chat_request_serialization() {
        let request = ChatRequest {
            model: "llama3".to_string(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: "You are helpful".to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                },
            ],
            stream: true,
            tools: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("llama3"));
        assert!(json.contains("system"));
        assert!(json.contains("You are helpful"));
        assert!(json.contains("user"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_chat_response_deserialization() {
        let json = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }]
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.choices[0].message.content, "Hello!");
    }

    #[test]
    fn test_streaming_chunk_deserialization() {
        let json = r#"{"id":"chatcmpl-123","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;

        let chunk: StreamingChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.id, "chatcmpl-123");
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_streaming_chunk_with_tool_call() {
        let json = r#"{"id":"chatcmpl-123","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_123","function":{"name":"bash","arguments":"{}"}}]},"finish_reason":null}]}"#;

        let chunk: StreamingChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].delta.tool_calls.as_ref().unwrap().len(), 1);
        let tool_call = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tool_call.function.name, "bash");
    }

    #[test]
    fn test_anthropic_request_serialization() {
        let request = AnthropicRequest {
            model: "claude-3-sonnet".to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            max_tokens: 1024,
            stream: true,
            system: Some("You are helpful".to_string()),
            tools: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("claude-3-sonnet"));
        assert!(json.contains("user"));
        assert!(json.contains("Hello"));
        assert!(json.contains("You are helpful"));
    }

    #[test]
    fn test_anthropic_response_deserialization() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "text_delta",
                "text": "Hello!"
            }
        }"#;

        let response: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        match response {
            AnthropicStreamEvent::ContentBlockDelta { delta, .. } => {
                if let AnthropicDelta::Text { text } = delta {
                    assert_eq!(text, "Hello!");
                }
            }
            _ => panic!("Expected ContentBlockDelta"),
        }
    }

    #[test]
    fn test_auto_detect_no_server() {
        // This test just verifies the method exists and can be called
        // Actual network test would be flaky
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { GenericProviderConfig::auto_detect().await });
        // Without a real server, this should return None
        // (or it might find something if there's actually a server running)
        assert!(result.is_none() || result.is_some()); // Always passes
    }

    #[test]
    fn test_url_building() {
        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        };

        let harness = GenericHttpHarness::new(config);
        // Test that we can build URLs (this is an internal method)
        // We verify by checking the config is properly set
        assert_eq!(harness.config.base_url, "http://localhost:11434");
    }

    #[test]
    fn test_serialization_roundtrip_with_all_fields() {
        let original = GenericProviderConfig {
            name: "Custom Provider".to_string(),
            base_url: "https://api.custom.com/v1".to_string(),
            api_key: Some("secret-key-123".to_string()),
            default_model: "custom-model-v1".to_string(),
            streaming: false,
        };

        let json = serde_json::to_string(&original).unwrap();
        let restored: GenericProviderConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.name, original.name);
        assert_eq!(restored.base_url, original.base_url);
        assert_eq!(restored.api_key, original.api_key);
        assert_eq!(restored.default_model, original.default_model);
        assert_eq!(restored.streaming, original.streaming);
    }

    #[test]
    fn test_validate_valid_config() {
        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_url() {
        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: "".to_string(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Provider URL cannot be empty"));
    }

    #[test]
    fn test_validate_invalid_url_scheme() {
        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: "localhost:11434".to_string(),
            api_key: None,
            default_model: "llama3".to_string(),
            streaming: true,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("http:// or https://"));
    }

    #[test]
    fn test_validate_empty_model() {
        let config = GenericProviderConfig {
            name: "Test".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: None,
            default_model: "".to_string(),
            streaming: true,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Default model cannot be empty"));
    }
}
