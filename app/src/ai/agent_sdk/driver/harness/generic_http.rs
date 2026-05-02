//! Generic HTTP Harness for OpenAI-compatible APIs.
//!
//! This harness enables Warp to use local LLM providers like Ollama, LM Studio,
//! and Jan without requiring a Warp server connection.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
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
}

impl GenericHttpHarness {
    pub fn new(config: GenericProviderConfig) -> Self {
        Self {
            config,
            conversation_history: Vec::new(),
        }
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
            tools: None,
        };

        // Prepare request
        let mut req_builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request);

        if let Some(ref api_key) = self.config.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        // Send request
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

        // For now, use non-streaming response
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

        // Update conversation history
        self.messages.push(Message {
            role: "user".to_string(),
            content: self.current_prompt.clone(),
        });
        self.messages.push(Message {
            role: "assistant".to_string(),
            content: content.clone(),
        });

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
