use crate::error::JanitorError;
use std::process::Command;

pub trait CommandRunner {
    fn run(&self, command: &str, args: &[&str]) -> Result<String, JanitorError>;
}

pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, command: &str, args: &[&str]) -> Result<String, JanitorError> {
        let output = Command::new(command).args(args).output().map_err(|e| {
            JanitorError::Command(format!("Failed to execute '{}': {}", command, e))
        })?;

        if !output.status.success() {
            return Err(JanitorError::Command(format!(
                "'{}' command failed: {}",
                command,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(String::from_utf8(output.stdout)
            .map_err(|e| {
                JanitorError::Command(format!("'{}' output is not valid UTF-8: {}", command, e))
            })?
            .trim()
            .to_string())
    }
}
