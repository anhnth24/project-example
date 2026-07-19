//! Converter sandbox resource limits.

use std::time::Duration;

/// Conservative process-level limits for untrusted document conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimits {
    pub wall_timeout: Duration,
    pub memory_bytes: u64,
    pub cpu_seconds: u64,
    pub file_size_bytes: u64,
    pub max_processes: u64,
    pub max_open_files: u64,
    pub stdout_stderr_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            wall_timeout: Duration::from_secs(120),
            memory_bytes: 768 * 1024 * 1024,
            cpu_seconds: 60,
            file_size_bytes: 64 * 1024 * 1024,
            max_processes: 512,
            max_open_files: 256,
            stdout_stderr_bytes: 16 * 1024 * 1024,
        }
    }
}

impl ResourceLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.wall_timeout.is_zero() || self.wall_timeout > Duration::from_secs(60 * 60) {
            return Err("converter wall timeout must be between 1 second and 1 hour".into());
        }
        if self.memory_bytes < 8 * 1024 * 1024 {
            return Err("converter memory limit must be at least 8 MiB".into());
        }
        if self.cpu_seconds == 0 || self.cpu_seconds > 60 * 60 {
            return Err("converter CPU limit must be between 1 second and 1 hour".into());
        }
        if self.file_size_bytes == 0 {
            return Err("converter file-size limit must be positive".into());
        }
        if self.max_processes == 0 || self.max_processes > 1024 {
            return Err("converter process limit must be between 1 and 1024".into());
        }
        if self.max_open_files < 8 || self.max_open_files > 4096 {
            return Err("converter open-file limit must be between 8 and 4096".into());
        }
        if self.stdout_stderr_bytes < 1024 {
            return Err("converter captured output limit must be at least 1024 bytes".into());
        }
        Ok(())
    }
}
