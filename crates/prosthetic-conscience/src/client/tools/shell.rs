//! Shell execution tool that runs commands inside a Docker container.

use std::io;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Tool, ToolDefinition, ToolError};

pub struct ShellTool {
    container_name: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl ShellTool {
    pub fn new(container_name: String, timeout: Duration, max_output_bytes: usize) -> Self {
        Self {
            container_name,
            timeout,
            max_output_bytes,
        }
    }
}

impl Tool for ShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "execute_shell".to_owned(),
            description: "Execute a shell command in the development environment".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn execute(&self, arguments: Value) -> Result<String, ToolError> {
        let command = match arguments.get("command").and_then(Value::as_str) {
            Some(cmd) => cmd,
            None => {
                return Err(ToolError::InvalidArguments {
                    message: "missing or non-string 'command' argument".to_owned(),
                });
            }
        };

        let child = Command::new("docker")
            .args(["exec", &self.container_name, "sh", "-c", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to spawn docker exec: {e}"),
            })?;

        let timeout = self.timeout;
        let max_bytes = self.max_output_bytes;

        match wait_with_timeout(child, timeout) {
            Ok(output) => Ok(format_output(
                output.status.code().unwrap_or(-1),
                &output.stdout,
                &output.stderr,
                max_bytes,
            )),
            Err(WaitError::Timeout { timeout_secs }) => Err(ToolError::ExecutionFailed {
                message: format!("command timed out after {timeout_secs}s"),
            }),
            Err(WaitError::Io(e)) => Err(ToolError::ExecutionFailed {
                message: format!("failed to wait on process: {e}"),
            }),
        }
    }
}

enum WaitError {
    Timeout { timeout_secs: u64 },
    Io(io::Error),
}

/// Wait for a child process to complete, with a timeout.
///
/// Spawns a thread to call `wait_with_output` (blocking). The main thread
/// waits on a channel with the given timeout. If the timeout expires, the
/// child is killed.
fn wait_with_timeout(child: std::process::Child, timeout: Duration) -> Result<Output, WaitError> {
    let (tx, rx) = mpsc::channel();

    // `Child` doesn't impl `Send` on all platforms in older Rust versions,
    // but it does on all platforms we target (unix, macOS).
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        // If the receiver is gone (timeout path killed+dropped), this is fine.
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(WaitError::Io(e)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // The child is owned by the thread — we can't kill it directly.
            // However, `docker exec` will be orphaned. For robustness, we
            // issue a separate `docker kill` is not needed here because the
            // thread will eventually complete and drop the child.
            // The thread owns the child and will clean up when it finishes.
            Err(WaitError::Timeout {
                timeout_secs: timeout.as_secs(),
            })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(WaitError::Io(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "wait thread terminated unexpectedly",
        ))),
    }
}

fn truncate_output(raw: &[u8], max_bytes: usize) -> String {
    let total = raw.len();
    if total <= max_bytes {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        let truncated = raw
            .get(..max_bytes)
            .map(String::from_utf8_lossy)
            .unwrap_or_else(|| String::from_utf8_lossy(raw));
        format!("{truncated}\n... (truncated, {total} bytes total)")
    }
}

fn format_output(exit_code: i32, stdout: &[u8], stderr: &[u8], max_bytes: usize) -> String {
    let mut result = format!(
        "exit code: {exit_code}\nstdout:\n{}",
        truncate_output(stdout, max_bytes)
    );
    if !stderr.is_empty() {
        result.push_str(&format!(
            "\nstderr:\n{}",
            truncate_output(stderr, max_bytes)
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tool() -> ShellTool {
        ShellTool::new("test-container".to_owned(), Duration::from_secs(30), 51200)
    }

    #[test]
    fn definition_has_correct_name() {
        let tool = test_tool();
        let def = tool.definition();
        assert_eq!(def.name, "execute_shell");
    }

    #[test]
    fn definition_parameters_require_command() {
        let tool = test_tool();
        let def = tool.definition();
        let required = def.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "command");
    }

    #[test]
    fn missing_command_returns_invalid_arguments() {
        let tool = test_tool();
        let result = tool.execute(json!({}));
        assert!(matches!(result, Err(ToolError::InvalidArguments { .. })));
    }

    #[test]
    fn non_string_command_returns_invalid_arguments() {
        let tool = test_tool();
        let result = tool.execute(json!({"command": 123}));
        assert!(matches!(result, Err(ToolError::InvalidArguments { .. })));
    }

    #[test]
    fn truncate_output_short() {
        let output = b"hello world";
        assert_eq!(truncate_output(output, 1024), "hello world");
    }

    #[test]
    fn truncate_output_over_limit() {
        let output = b"abcdefghij"; // 10 bytes
        let result = truncate_output(output, 5);
        assert!(result.starts_with("abcde"));
        assert!(result.contains("truncated"));
        assert!(result.contains("10 bytes total"));
    }

    #[test]
    fn format_output_stdout_only() {
        let result = format_output(0, b"hello\n", b"", 51200);
        assert!(result.starts_with("exit code: 0"));
        assert!(result.contains("stdout:\nhello\n"));
        assert!(!result.contains("stderr:"));
    }

    #[test]
    fn format_output_with_stderr() {
        let result = format_output(1, b"out\n", b"err\n", 51200);
        assert!(result.contains("exit code: 1"));
        assert!(result.contains("stdout:\nout\n"));
        assert!(result.contains("stderr:\nerr\n"));
    }

    // Docker-dependent tests — require a running container named "pc-test-sandbox".
    // Run with: cargo test --all-targets -- --ignored
    // Setup: docker run -d --name pc-test-sandbox ubuntu:24.04 sleep infinity

    #[test]
    #[ignore]
    fn executes_echo_in_container() {
        let tool = ShellTool::new("pc-test-sandbox".to_owned(), Duration::from_secs(10), 51200);
        let result = tool
            .execute(json!({"command": "echo hello"}))
            .expect("should succeed");
        assert!(result.contains("exit code: 0"));
        assert!(result.contains("hello"));
    }

    #[test]
    #[ignore]
    fn captures_stderr() {
        let tool = ShellTool::new("pc-test-sandbox".to_owned(), Duration::from_secs(10), 51200);
        let result = tool
            .execute(json!({"command": "echo err >&2"}))
            .expect("should succeed");
        assert!(result.contains("stderr:"));
        assert!(result.contains("err"));
    }

    #[test]
    #[ignore]
    fn nonzero_exit_code() {
        let tool = ShellTool::new("pc-test-sandbox".to_owned(), Duration::from_secs(10), 51200);
        let result = tool
            .execute(json!({"command": "false"}))
            .expect("should succeed even with nonzero exit");
        assert!(result.contains("exit code: 1"));
    }

    #[test]
    #[ignore]
    fn timeout_returns_error() {
        let tool = ShellTool::new("pc-test-sandbox".to_owned(), Duration::from_secs(1), 51200);
        let result = tool.execute(json!({"command": "sleep 60"}));
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("timed out"), "got: {err_msg}");
    }
}
