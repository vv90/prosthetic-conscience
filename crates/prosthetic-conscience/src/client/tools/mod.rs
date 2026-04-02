//! Tool trait, registry, and error types for client-side tool execution.

pub mod current_time;
pub mod shell;

use std::collections::HashMap;

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    fn execute(&self, arguments: Value) -> Result<String, ToolError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("invalid arguments: {message}")]
    InvalidArguments { message: String },
    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.definition().name;
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Returns the `tools` array for inclusion in chat request payloads.
    ///
    /// Each entry is in OpenAI format:
    /// `{type: "function", function: {name, description, parameters}}`
    pub fn definitions(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|tool| {
                let def = tool.definition();
                json!({
                    "type": "function",
                    "function": {
                        "name": def.name,
                        "description": def.description,
                        "parameters": def.parameters,
                    }
                })
            })
            .collect()
    }

    /// Look up a tool by name and execute it.
    pub fn execute(&self, name: &str, arguments: Value) -> Result<String, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| ToolError::UnknownTool {
            name: name.to_owned(),
        })?;
        tool.execute(arguments)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct EchoTool;

    impl Tool for EchoTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "echo".to_owned(),
                description: "Echoes the input".to_owned(),
                parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            }
        }

        fn execute(&self, arguments: Value) -> Result<String, ToolError> {
            let text = arguments
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("(empty)");
            Ok(text.to_owned())
        }
    }

    #[test]
    fn register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn execute_known_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let result = registry
            .execute("echo", json!({"text": "hello"}))
            .expect("should succeed");
        assert_eq!(result, "hello");
    }

    #[test]
    fn execute_unknown_tool_returns_error() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", json!({}));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ToolError::UnknownTool { name } if name == "nonexistent"
        ));
    }

    #[test]
    fn definitions_returns_openai_format() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let defs = registry.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["type"], "function");
        assert_eq!(defs[0]["function"]["name"], "echo");
        assert_eq!(defs[0]["function"]["description"], "Echoes the input");
        assert!(defs[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn duplicate_registration_overwrites() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(EchoTool));
        assert_eq!(registry.definitions().len(), 1);
    }
}
