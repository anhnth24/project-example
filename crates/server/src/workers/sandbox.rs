//! Generic isolated command runner for converter workers.
//!
//! The heavy converter remains outside `fileconv-server`; this module only
//! materializes a single input file and executes a configured argv template.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::limits::ResourceLimits;

const INPUT_PLACEHOLDER: &str = "{input}";
const POLL_INTERVAL: Duration = Duration::from_millis(20);

const ACCESS_FS_EXECUTE: u64 = 1 << 0;
const ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const ACCESS_FS_READ_FILE: u64 = 1 << 2;
const ACCESS_FS_READ_DIR: u64 = 1 << 3;
const ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
const ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;
const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
const LANDLOCK_ACCESS_FS_READ_EXECUTE: u64 =
    ACCESS_FS_EXECUTE | ACCESS_FS_READ_FILE | ACCESS_FS_READ_DIR;
const LANDLOCK_ACCESS_FS_ALL_V1: u64 = ACCESS_FS_EXECUTE
    | ACCESS_FS_WRITE_FILE
    | ACCESS_FS_READ_FILE
    | ACCESS_FS_READ_DIR
    | ACCESS_FS_REMOVE_DIR
    | ACCESS_FS_REMOVE_FILE
    | ACCESS_FS_MAKE_CHAR
    | ACCESS_FS_MAKE_DIR
    | ACCESS_FS_MAKE_REG
    | ACCESS_FS_MAKE_SOCK
    | ACCESS_FS_MAKE_FIFO
    | ACCESS_FS_MAKE_BLOCK
    | ACCESS_FS_MAKE_SYM;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub argv_template: Vec<String>,
    pub limits: ResourceLimits,
}

impl SandboxConfig {
    pub fn validate(&self) -> Result<(), SandboxError> {
        if self.argv_template.is_empty() {
            return Err(SandboxError::InvalidConfig(
                "converter argv template must not be empty".into(),
            ));
        }
        let placeholder_count = self
            .argv_template
            .iter()
            .filter(|arg| arg.contains(INPUT_PLACEHOLDER))
            .count();
        if placeholder_count == 0 {
            return Err(SandboxError::InvalidConfig(
                "converter argv template must contain {input}".into(),
            ));
        }
        self.limits.validate().map_err(SandboxError::InvalidConfig)
    }
}

#[derive(Debug)]
pub struct SandboxInput {
    pub bytes: Vec<u8>,
    pub canonical_extension: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxExit {
    Success,
    Exit(i32),
    Signaled(i32),
    TimedOut,
    Cancelled,
}

impl SandboxExit {
    pub const fn success(self) -> bool {
        matches!(self, Self::Success)
    }
}

#[derive(Debug)]
pub struct SandboxOutput {
    pub exit: SandboxExit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Path retained for tests/evidence; RAII cleanup has removed it by return.
    pub workspace_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("sandbox isolation is unavailable")]
    IsolationUnavailable,
    #[error("sandbox io failed")]
    Io(#[from] io::Error),
}

#[derive(Clone, Debug, Default)]
pub struct SandboxCancel {
    cancelled: Arc<AtomicBool>,
}

impl SandboxCancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Probe isolation support with a tiny command. Workers call this at startup and
/// tests use it to skip only when the kernel truly lacks unprivileged isolation.
pub fn preflight() -> Result<(), SandboxError> {
    let config = SandboxConfig {
        argv_template: vec!["/bin/true".into(), INPUT_PLACEHOLDER.into()],
        limits: ResourceLimits {
            wall_timeout: Duration::from_secs(5),
            memory_bytes: 64 * 1024 * 1024,
            cpu_seconds: 2,
            file_size_bytes: 1024 * 1024,
            max_processes: 512,
            max_open_files: 32,
            stdout_stderr_bytes: 1024 * 1024,
        },
    };
    let output = run(
        &config,
        SandboxInput {
            bytes: Vec::new(),
            canonical_extension: "txt".into(),
        },
        &SandboxCancel::default(),
    )?;
    if output.exit.success() {
        Ok(())
    } else {
        Err(SandboxError::IsolationUnavailable)
    }
}

pub fn run(
    config: &SandboxConfig,
    input: SandboxInput,
    cancel: &SandboxCancel,
) -> Result<SandboxOutput, SandboxError> {
    config.validate()?;
    let workspace = TempDir::new()?;
    let workspace_path = workspace.path().to_path_buf();
    let input_name = safe_input_name(&input.canonical_extension)?;
    let input_path = workspace.path().join(input_name);
    {
        let mut file = File::create(&input_path)?;
        file.write_all(&input.bytes)?;
        file.sync_all()?;
    }

    let argv = materialize_argv(&config.argv_template, &input_path)?;
    let executable = argv
        .first()
        .ok_or_else(|| SandboxError::InvalidConfig("converter argv is empty".into()))?;
    let pre_exec = PreExecConfig::new(config, workspace.path(), executable)?;

    let mut command = Command::new(executable);
    command
        .args(&argv[1..])
        .current_dir(workspace.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    unsafe {
        command.pre_exec(move || pre_exec.apply());
    }

    let mut child = command.spawn()?;
    let child_pid = child.id() as libc::pid_t;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let stdout_limit = config.limits.stdout_stderr_bytes;
    let stderr_limit = config.limits.stdout_stderr_bytes;
    let stdout_reader = thread::spawn(move || read_capped(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_capped(stderr, stderr_limit));

    let deadline = Instant::now() + config.limits.wall_timeout;
    let exit = loop {
        if let Some(status) = child.try_wait()? {
            break exit_from_status(status);
        }
        if cancel.is_cancelled() {
            kill_process_tree(child_pid);
            let _ = child.wait();
            break SandboxExit::Cancelled;
        }
        if Instant::now() >= deadline {
            kill_process_tree(child_pid);
            let _ = child.wait();
            break SandboxExit::TimedOut;
        }
        thread::sleep(POLL_INTERVAL);
    };

    let stdout = join_reader(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    drop(workspace);
    Ok(SandboxOutput {
        exit,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        workspace_path,
    })
}

fn materialize_argv(template: &[String], input_path: &Path) -> Result<Vec<String>, SandboxError> {
    let input = input_path
        .to_str()
        .ok_or_else(|| SandboxError::InvalidConfig("input path is not UTF-8".into()))?;
    Ok(template
        .iter()
        .map(|arg| arg.replace(INPUT_PLACEHOLDER, input))
        .collect())
}

fn safe_input_name(extension: &str) -> Result<String, SandboxError> {
    let ext = extension.trim_start_matches('.').to_ascii_lowercase();
    if ext.is_empty()
        || ext.len() > 16
        || !ext
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(SandboxError::InvalidConfig(
            "canonical extension is invalid".into(),
        ));
    }
    Ok(format!("input.{ext}"))
}

struct CapturedPipe {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_capped<R: Read>(mut pipe: R, limit: usize) -> io::Result<CapturedPipe> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut buf = [0_u8; 8192];
    loop {
        let n = pipe.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let allowed = remaining.min(n);
        if allowed > 0 {
            bytes.extend_from_slice(&buf[..allowed]);
        }
        if allowed < n {
            truncated = true;
        }
    }
    Ok(CapturedPipe { bytes, truncated })
}

fn join_reader(handle: thread::JoinHandle<io::Result<CapturedPipe>>) -> io::Result<CapturedPipe> {
    handle
        .join()
        .map_err(|_| io::Error::other("sandbox pipe reader panicked"))?
}

fn exit_from_status(status: std::process::ExitStatus) -> SandboxExit {
    if status.success() {
        SandboxExit::Success
    } else if let Some(code) = status.code() {
        SandboxExit::Exit(code)
    } else if let Some(signal) = status.signal() {
        SandboxExit::Signaled(signal)
    } else {
        SandboxExit::Exit(-1)
    }
}

fn kill_process_tree(pid: libc::pid_t) {
    unsafe {
        let _ = libc::kill(-pid, libc::SIGKILL);
        let _ = libc::kill(pid, libc::SIGKILL);
    }
}

struct PreExecConfig {
    limits: ResourceLimits,
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    setgroups: CString,
    uid_map_path: CString,
    gid_map_path: CString,
    root_path: CString,
    landlock_allow: Vec<(CString, u64)>,
}

impl PreExecConfig {
    fn new(config: &SandboxConfig, workspace: &Path, executable: &str) -> io::Result<Self> {
        let workspace = CString::new(path_to_bytes(workspace)?)?;
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let executable_dir = executable_allow_path(executable);
        let mut landlock_allow = vec![
            (workspace.clone(), LANDLOCK_ACCESS_FS_ALL_V1),
            (cstring_path("/bin")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
            (cstring_path("/usr/bin")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
            (cstring_path("/lib")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
            (cstring_path("/lib64")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
            (cstring_path("/usr/lib")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
            (cstring_path("/usr/lib64")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
            (cstring_path("/etc/ld.so.cache")?, ACCESS_FS_READ_FILE),
        ];
        if let Some(path) = executable_dir {
            landlock_allow.push((CString::new(path)?, LANDLOCK_ACCESS_FS_READ_EXECUTE));
        }
        Ok(Self {
            limits: config.limits.clone(),
            uid_map: format!("0 {uid} 1\n").into_bytes(),
            gid_map: format!("0 {gid} 1\n").into_bytes(),
            setgroups: cstring_path("/proc/self/setgroups")?,
            uid_map_path: cstring_path("/proc/self/uid_map")?,
            gid_map_path: cstring_path("/proc/self/gid_map")?,
            root_path: cstring_path("/")?,
            landlock_allow,
        })
    }

    fn apply(&self) -> io::Result<()> {
        unsafe {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        apply_rlimit(libc::RLIMIT_AS, self.limits.memory_bytes)?;
        apply_rlimit(libc::RLIMIT_CPU, self.limits.cpu_seconds)?;
        apply_rlimit(libc::RLIMIT_FSIZE, self.limits.file_size_bytes)?;
        apply_rlimit(libc::RLIMIT_NPROC, self.limits.max_processes)?;
        apply_rlimit(libc::RLIMIT_CORE, 0)?;
        self.unshare_user_and_network()?;
        self.apply_landlock()?;
        apply_rlimit(libc::RLIMIT_NOFILE, self.limits.max_open_files)?;
        Ok(())
    }

    fn unshare_user_and_network(&self) -> io::Result<()> {
        unsafe {
            if libc::unshare(libc::CLONE_NEWUSER) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        write_all_raw(&self.setgroups, b"deny\n")?;
        write_all_raw(&self.uid_map_path, &self.uid_map)?;
        write_all_raw(&self.gid_map_path, &self.gid_map)?;
        unsafe {
            if libc::unshare(libc::CLONE_NEWNET) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::unshare(libc::CLONE_NEWNS) == 0 {
                let _ = libc::mount(
                    std::ptr::null(),
                    self.root_path.as_ptr(),
                    std::ptr::null(),
                    (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
                    std::ptr::null(),
                );
            }
        }
        Ok(())
    }

    fn apply_landlock(&self) -> io::Result<()> {
        unsafe {
            let abi = libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0,
                LANDLOCK_CREATE_RULESET_VERSION,
            );
            if abi < 1 {
                return Err(io::Error::last_os_error());
            }
            let ruleset_attr = LandlockRulesetAttr {
                handled_access_fs: LANDLOCK_ACCESS_FS_ALL_V1,
            };
            let ruleset_fd = libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &ruleset_attr as *const LandlockRulesetAttr,
                std::mem::size_of::<LandlockRulesetAttr>(),
                0,
            ) as RawFd;
            if ruleset_fd < 0 {
                return Err(io::Error::last_os_error());
            }
            for (path, access) in &self.landlock_allow {
                let fd = libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC);
                if fd < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
                        continue;
                    }
                    let error = io::Error::last_os_error();
                    let _ = libc::close(ruleset_fd);
                    return Err(error);
                }
                let rule = LandlockPathBeneathAttr {
                    allowed_access: *access,
                    parent_fd: fd,
                };
                let add_result = libc::syscall(
                    libc::SYS_landlock_add_rule,
                    ruleset_fd,
                    LANDLOCK_RULE_PATH_BENEATH,
                    &rule as *const LandlockPathBeneathAttr,
                    0,
                );
                let close_result = libc::close(fd);
                if add_result < 0 {
                    let error = io::Error::last_os_error();
                    let _ = libc::close(ruleset_fd);
                    return Err(error);
                }
                if close_result < 0 {
                    let error = io::Error::last_os_error();
                    let _ = libc::close(ruleset_fd);
                    return Err(error);
                }
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                let error = io::Error::last_os_error();
                let _ = libc::close(ruleset_fd);
                return Err(error);
            }
            let restrict_result = libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0);
            let close_result = libc::close(ruleset_fd);
            if restrict_result < 0 {
                return Err(io::Error::last_os_error());
            }
            if close_result < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

fn apply_rlimit(resource: libc::__rlimit_resource_t, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    unsafe {
        if libc::setrlimit(resource, &limit) < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn write_all_raw(path: &CString, bytes: &[u8]) -> io::Result<()> {
    unsafe {
        let fd = libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_TRUNC,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut written = 0;
        while written < bytes.len() {
            let n = libc::write(
                fd,
                bytes[written..].as_ptr().cast::<libc::c_void>(),
                bytes.len() - written,
            );
            if n < 0 {
                let error = io::Error::last_os_error();
                let _ = libc::close(fd);
                return Err(error);
            }
            written += n as usize;
        }
        if libc::close(fd) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn cstring_path(path: &str) -> io::Result<CString> {
    CString::new(path).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul path"))
}

fn path_to_bytes(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "nul path"))
    } else {
        Ok(bytes.to_vec())
    }
}

fn executable_allow_path(executable: &str) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(executable);
    if !path.is_absolute() {
        return None;
    }
    let parent = path.parent()?;
    Some(parent.as_os_str().as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_config(script: &str, timeout: Duration) -> SandboxConfig {
        SandboxConfig {
            argv_template: vec![
                "/bin/sh".into(),
                "-c".into(),
                script.into(),
                "sh".into(),
                INPUT_PLACEHOLDER.into(),
            ],
            limits: ResourceLimits {
                wall_timeout: timeout,
                memory_bytes: 128 * 1024 * 1024,
                cpu_seconds: 5,
                file_size_bytes: 1024 * 1024,
                max_processes: 32,
                max_open_files: 32,
                stdout_stderr_bytes: 1024 * 1024,
            },
        }
    }

    fn input() -> SandboxInput {
        SandboxInput {
            bytes: b"hello".to_vec(),
            canonical_extension: "txt".into(),
        }
    }

    #[test]
    fn sandbox_cleans_workspace_after_success() {
        if preflight().is_err() {
            eprintln!("skipped: sandbox isolation unavailable");
            return;
        }
        let output = run(
            &shell_config("printf ok", Duration::from_secs(5)),
            input(),
            &SandboxCancel::default(),
        )
        .expect("sandbox run");
        assert_eq!(output.exit, SandboxExit::Success);
        assert_eq!(output.stdout, b"ok");
        assert!(!output.workspace_path.exists());
    }
}
