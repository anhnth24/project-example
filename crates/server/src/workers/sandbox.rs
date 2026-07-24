//! Generic isolated command runner for converter workers.
//!
//! The heavy converter remains outside `fileconv-server`; this module only
//! materializes a single input file and executes a configured argv template.

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::os::fd::AsRawFd;
    use std::os::fd::RawFd;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::super::limits::ResourceLimits;

    const INPUT_PLACEHOLDER: &str = "{input}";
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    const DRAIN_GRACE: Duration = Duration::from_millis(250);
    const KILL_REAP_GRACE: Duration = Duration::from_millis(500);

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
    const LANDLOCK_ACCESS_FS_EXEC_FILE: u64 = ACCESS_FS_EXECUTE | ACCESS_FS_READ_FILE;
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
            if !Path::new(&self.argv_template[0]).is_absolute() {
                return Err(SandboxError::InvalidConfig(
                    "converter executable must be an absolute path".into(),
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
        /// Host PID of PID namespace init. It is gone after timeout/cancel cleanup.
        pub pid1_host_pid: Option<u32>,
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
        let (pid_read_fd, pid_write_fd) = pipe_cloexec()?;
        let pre_exec = PreExecConfig::new(
            config,
            workspace.path(),
            executable,
            pid_read_fd,
            pid_write_fd,
        )?;

        let mut command = Command::new(executable);
        command
            .args(&argv[1..])
            .current_dir(workspace.path())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LC_ALL", "C")
            // Landlock grants writes only inside this per-job workspace. Keep
            // converter/OCR temporary files there instead of denied host /tmp.
            .env("TMPDIR", workspace.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Allowlist-only passthrough so the converter can load pinned native deps
        // (PDFium / Tesseract) without inheriting worker secrets.
        for key in [
            "FILECONV_PDFIUM_LIB",
            "FILECONV_TESSDATA",
            "TESSDATA_PREFIX",
            "LANG",
        ] {
            if let Ok(value) = std::env::var(key) {
                if !value.is_empty() {
                    command.env(key, value);
                }
            }
        }

        unsafe {
            command.pre_exec(move || pre_exec.apply());
        }

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                close_fd(pid_read_fd);
                close_fd(pid_write_fd);
                return Err(error.into());
            }
        };
        close_fd(pid_write_fd);
        let child_pid = child.id() as libc::pid_t;
        let mut supervisor = SandboxSupervisor::new(child, child_pid);
        let pid1_host_pid = match read_pid_from_pipe(pid_read_fd) {
            Ok(Some(pid)) => pid,
            Ok(None) => {
                close_fd(pid_read_fd);
                return Err(SandboxError::IsolationUnavailable);
            }
            Err(error) => {
                close_fd(pid_read_fd);
                return Err(SandboxError::Io(error));
            }
        };
        supervisor.set_pid1(pid1_host_pid);
        close_fd(pid_read_fd);
        let cgroup_guard = CgroupGuard::best_effort_apply(pid1_host_pid, &config.limits);
        let stdout = supervisor.child_mut().stdout.take().expect("stdout piped");
        let stderr = supervisor.child_mut().stderr.take().expect("stderr piped");
        set_nonblocking(stdout.as_raw_fd())?;
        set_nonblocking(stderr.as_raw_fd())?;
        let mut stdout_capture = CapturedPipe::new(config.limits.stdout_stderr_bytes);
        let mut stderr_capture = CapturedPipe::new(config.limits.stdout_stderr_bytes);

        let deadline = Instant::now() + config.limits.wall_timeout;
        let exit = loop {
            drain_available(stdout.as_raw_fd(), &mut stdout_capture)?;
            drain_available(stderr.as_raw_fd(), &mut stderr_capture)?;
            if let Some(status) = supervisor.child_mut().try_wait()? {
                supervisor.disarm();
                break exit_from_status(status);
            }
            if cancel.is_cancelled() {
                supervisor.kill_and_reap()?;
                break SandboxExit::Cancelled;
            }
            if Instant::now() >= deadline {
                supervisor.kill_and_reap()?;
                break SandboxExit::TimedOut;
            }
            std::thread::sleep(POLL_INTERVAL);
        };

        let drain_deadline = Instant::now() + DRAIN_GRACE;
        while Instant::now() < drain_deadline && (!stdout_capture.eof || !stderr_capture.eof) {
            drain_available(stdout.as_raw_fd(), &mut stdout_capture)?;
            drain_available(stderr.as_raw_fd(), &mut stderr_capture)?;
            if stdout_capture.eof && stderr_capture.eof {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        if !stdout_capture.eof {
            stdout_capture.truncated = true;
        }
        if !stderr_capture.eof {
            stderr_capture.truncated = true;
        }
        drop(cgroup_guard);
        drop(workspace);
        Ok(SandboxOutput {
            exit,
            stdout: stdout_capture.bytes,
            stderr: stderr_capture.bytes,
            stdout_truncated: stdout_capture.truncated,
            stderr_truncated: stderr_capture.truncated,
            pid1_host_pid: Some(pid1_host_pid),
            workspace_path,
        })
    }

    fn materialize_argv(
        template: &[String],
        input_path: &Path,
    ) -> Result<Vec<String>, SandboxError> {
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
        limit: usize,
        truncated: bool,
        eof: bool,
    }

    impl CapturedPipe {
        fn new(limit: usize) -> Self {
            Self {
                bytes: Vec::with_capacity(limit.min(8192)),
                limit,
                truncated: false,
                eof: false,
            }
        }

        fn capacity_remaining(&self) -> usize {
            self.limit.saturating_sub(self.bytes.len())
        }
    }

    fn drain_available(fd: RawFd, capture: &mut CapturedPipe) -> io::Result<()> {
        let mut buf = [0_u8; 8192];
        loop {
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast::<libc::c_void>(), buf.len()) };
            if n < 0 {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error(),
                    Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
                ) {
                    return Ok(());
                }
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(error);
            }
            if n == 0 {
                capture.eof = true;
                return Ok(());
            }
            let n = n as usize;
            let allowed = capture.capacity_remaining().min(n);
            if allowed > 0 {
                capture.bytes.extend_from_slice(&buf[..allowed]);
            }
            if allowed < n {
                capture.truncated = true;
            }
        }
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

    fn reap_child_bounded(child: &mut Child, grace: Duration) -> io::Result<()> {
        let deadline = Instant::now() + grace;
        loop {
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "sandbox child did not reap after kill",
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    struct SandboxSupervisor {
        child: Option<Child>,
        wrapper_pid: libc::pid_t,
        pid1_host_pid: Option<u32>,
    }

    impl SandboxSupervisor {
        fn new(child: Child, wrapper_pid: libc::pid_t) -> Self {
            Self {
                child: Some(child),
                wrapper_pid,
                pid1_host_pid: None,
            }
        }

        fn child_mut(&mut self) -> &mut Child {
            self.child.as_mut().expect("sandbox child present")
        }

        fn set_pid1(&mut self, pid1_host_pid: u32) {
            self.pid1_host_pid = Some(pid1_host_pid);
        }

        fn disarm(&mut self) {
            self.child = None;
        }

        fn kill_and_reap(&mut self) -> io::Result<()> {
            let Some(child) = self.child.as_mut() else {
                return Ok(());
            };
            kill_and_reap_child(child, self.pid1_host_pid, self.wrapper_pid)?;
            self.child = None;
            Ok(())
        }
    }

    impl Drop for SandboxSupervisor {
        fn drop(&mut self) {
            let _ = self.kill_and_reap();
        }
    }

    fn kill_and_reap_child(
        child: &mut Child,
        pid1_host_pid: Option<u32>,
        wrapper_pid: libc::pid_t,
    ) -> io::Result<()> {
        if let Some(pid1) = pid1_host_pid {
            unsafe {
                let _ = libc::kill(pid1 as libc::pid_t, libc::SIGKILL);
            }
        }
        if reap_child_bounded(child, KILL_REAP_GRACE).is_ok() {
            return Ok(());
        }
        unsafe {
            let _ = libc::kill(wrapper_pid, libc::SIGKILL);
        }
        if reap_child_bounded(child, KILL_REAP_GRACE).is_ok() {
            return Ok(());
        }
        if let Some(pid1) = pid1_host_pid {
            unsafe {
                let _ = libc::kill(pid1 as libc::pid_t, libc::SIGKILL);
            }
        }
        unsafe {
            let _ = libc::kill(wrapper_pid, libc::SIGKILL);
        }
        reap_child_bounded(child, KILL_REAP_GRACE)
    }

    struct PreExecConfig {
        limits: ResourceLimits,
        pid_read_fd: RawFd,
        pid_write_fd: RawFd,
        uid_map: Vec<u8>,
        gid_map: Vec<u8>,
        setgroups: CString,
        uid_map_path: CString,
        gid_map_path: CString,
        root_path: CString,
        landlock_allow: Vec<(CString, u64)>,
    }

    impl PreExecConfig {
        fn new(
            config: &SandboxConfig,
            workspace: &Path,
            executable: &str,
            pid_read_fd: RawFd,
            pid_write_fd: RawFd,
        ) -> io::Result<Self> {
            let workspace = CString::new(path_to_bytes(workspace)?)?;
            let uid = unsafe { libc::getuid() };
            let gid = unsafe { libc::getgid() };
            let executable_path = CString::new(path_to_bytes(Path::new(executable))?)?;
            let mut landlock_allow = vec![
                (workspace.clone(), LANDLOCK_ACCESS_FS_ALL_V1),
                (executable_path, LANDLOCK_ACCESS_FS_EXEC_FILE),
                // Preflight uses /bin/true; converter shells out to tesseract under /usr/bin.
                (cstring_path("/bin")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
                (cstring_path("/usr/bin")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
                (
                    cstring_path("/usr/local/bin")?,
                    LANDLOCK_ACCESS_FS_READ_EXECUTE,
                ),
                (cstring_path("/lib")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
                (cstring_path("/lib64")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
                (cstring_path("/usr/lib")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
                (cstring_path("/usr/lib64")?, LANDLOCK_ACCESS_FS_READ_EXECUTE),
                (cstring_path("/etc/ld.so.cache")?, ACCESS_FS_READ_FILE),
                // std::process::Command::output() opens /dev/null for child
                // stdin. Without this exact device rule nested OCR spawn gets
                // EACCES before exec, even though tesseract itself is allowed.
                (
                    cstring_path("/dev/null")?,
                    ACCESS_FS_READ_FILE | ACCESS_FS_WRITE_FILE,
                ),
                // Pinned PDFium + Debian Tesseract tessdata locations.
                (
                    cstring_path("/opt/pdfium")?,
                    LANDLOCK_ACCESS_FS_READ_EXECUTE,
                ),
                (
                    cstring_path("/usr/share/tesseract-ocr")?,
                    LANDLOCK_ACCESS_FS_READ_EXECUTE,
                ),
                (
                    cstring_path("/usr/share/tessdata")?,
                    LANDLOCK_ACCESS_FS_READ_EXECUTE,
                ),
            ];
            for key in [
                "FILECONV_PDFIUM_LIB",
                "FILECONV_TESSDATA",
                "TESSDATA_PREFIX",
            ] {
                if let Ok(value) = std::env::var(key) {
                    let trimmed = value.trim();
                    let path = Path::new(trimmed);
                    if path.is_absolute() {
                        if let Ok(c_path) = cstring_path(trimmed) {
                            landlock_allow.push((c_path, LANDLOCK_ACCESS_FS_READ_EXECUTE));
                        }
                        if let Some(parent) = path.parent().and_then(Path::to_str) {
                            if let Ok(c_path) = cstring_path(parent) {
                                landlock_allow.push((c_path, LANDLOCK_ACCESS_FS_READ_EXECUTE));
                            }
                        }
                    }
                }
            }
            Ok(Self {
                limits: config.limits.clone(),
                pid_read_fd,
                pid_write_fd,
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
            apply_rlimit(libc::RLIMIT_CORE, 0)?;
            self.unshare_user_and_network()?;
            self.apply_landlock()?;
            apply_rlimit(libc::RLIMIT_NOFILE, self.limits.max_open_files)?;
            self.enter_pid_namespace()?;
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
                if libc::unshare(libc::CLONE_NEWNS) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::mount(
                    std::ptr::null(),
                    self.root_path.as_ptr(),
                    std::ptr::null(),
                    (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
                    std::ptr::null(),
                ) < 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }

        fn enter_pid_namespace(&self) -> io::Result<()> {
            unsafe {
                if libc::unshare(libc::CLONE_NEWPID) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let pid = libc::fork();
                if pid < 0 {
                    return Err(io::Error::last_os_error());
                }
                if pid == 0 {
                    let _ = libc::close(self.pid_read_fd);
                    let _ = libc::close(self.pid_write_fd);
                    apply_rlimit(libc::RLIMIT_NPROC, self.limits.max_processes)?;
                    close_fds_from(3);
                    return Ok(());
                }

                let _ = libc::close(self.pid_read_fd);
                if !write_all_fd_raw(self.pid_write_fd, &(pid as u32).to_ne_bytes()) {
                    let _ = libc::kill(pid, libc::SIGKILL);
                    let mut status: libc::c_int = 0;
                    loop {
                        if libc::waitpid(pid, &mut status, 0) >= 0 {
                            break;
                        }
                        if errno_raw() != libc::EINTR {
                            break;
                        }
                    }
                    let _ = libc::close(self.pid_write_fd);
                    libc::_exit(127);
                }
                let _ = libc::close(self.pid_write_fd);
                close_fds_from(3);

                let mut status: libc::c_int = 0;
                loop {
                    if libc::waitpid(pid, &mut status, 0) >= 0 {
                        break;
                    }
                    if errno_raw() != libc::EINTR {
                        libc::_exit(127);
                    }
                }
                if libc::WIFEXITED(status) {
                    libc::_exit(libc::WEXITSTATUS(status));
                }
                if libc::WIFSIGNALED(status) {
                    libc::_exit(128 + libc::WTERMSIG(status));
                }
                libc::_exit(127);
            }
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
                let restrict_result =
                    libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0);
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

    fn pipe_cloexec() -> io::Result<(RawFd, RawFd)> {
        let mut fds = [0; 2];
        unsafe {
            if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok((fds[0], fds[1]))
            }
        }
    }

    fn close_fd(fd: RawFd) {
        unsafe {
            let _ = libc::close(fd);
        }
    }

    fn read_pid_from_pipe(fd: RawFd) -> io::Result<Option<u32>> {
        let mut bytes = [0_u8; 4];
        let mut read = 0;
        while read < bytes.len() {
            let n = unsafe {
                libc::read(
                    fd,
                    bytes[read..].as_mut_ptr().cast::<libc::c_void>(),
                    bytes.len() - read,
                )
            };
            if n < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(error);
            }
            if n == 0 {
                return Ok(None);
            }
            read += n as usize;
        }
        Ok(Some(u32::from_ne_bytes(bytes)))
    }

    fn set_nonblocking(fd: RawFd) -> io::Result<()> {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn write_all_fd_raw(fd: RawFd, bytes: &[u8]) -> bool {
        let mut written = 0;
        while written < bytes.len() {
            let n = unsafe {
                libc::write(
                    fd,
                    bytes[written..].as_ptr().cast::<libc::c_void>(),
                    bytes.len() - written,
                )
            };
            if n < 0 {
                if errno_raw() == libc::EINTR {
                    continue;
                }
                return false;
            }
            written += n as usize;
        }
        true
    }

    fn errno_raw() -> libc::c_int {
        unsafe { *libc::__errno_location() }
    }

    fn close_fds_from(first: RawFd) {
        unsafe {
            #[cfg(target_os = "linux")]
            {
                let result =
                    libc::syscall(libc::SYS_close_range, first as libc::c_uint, !0_u32, 0_u32);
                if result == 0 {
                    return;
                }
            }
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            let max = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
                limit.rlim_cur.min(4096) as RawFd
            } else {
                1024
            };
            for fd in first..max {
                let _ = libc::close(fd);
            }
        }
    }

    struct CgroupGuard {
        path: Option<PathBuf>,
    }

    impl CgroupGuard {
        // Container runtime (P1B-F02 compose) sets mem_limit/pids_limit on the
        // worker. This best-effort per-job cgroup only activates when a writable
        // delegated subtree exists inside the container.
        fn best_effort_apply(pid: u32, limits: &ResourceLimits) -> Self {
            match Self::try_apply(pid, limits) {
                Ok(path) => Self { path: Some(path) },
                Err(_) => Self { path: None },
            }
        }

        fn try_apply(pid: u32, limits: &ResourceLimits) -> io::Result<PathBuf> {
            let root = Path::new("/sys/fs/cgroup");
            if !root.join("cgroup.controllers").exists() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "cgroup v2 missing"));
            }
            let path = root.join(format!("markhand-convert-{pid}"));
            fs::create_dir(&path)?;
            fs::write(path.join("memory.max"), limits.memory_bytes.to_string())?;
            fs::write(path.join("pids.max"), limits.max_processes.to_string())?;
            fs::write(path.join("cgroup.procs"), pid.to_string())?;
            Ok(path)
        }
    }

    impl Drop for CgroupGuard {
        fn drop(&mut self) {
            if let Some(path) = self.path.take() {
                let _ = fs::remove_dir(path);
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
        #[ignore = "requires a built fileconv binary and Tesseract"]
        fn live_png_ocr_runs_inside_production_sandbox() {
            let tesseract_probe = run(
                &shell_config("/usr/bin/tesseract --version", Duration::from_secs(5)),
                input(),
                &SandboxCancel::default(),
            )
            .expect("tesseract probe sandbox");
            assert_eq!(
                tesseract_probe.exit,
                SandboxExit::Success,
                "tesseract probe stderr={}",
                String::from_utf8_lossy(&tesseract_probe.stderr)
            );
            let fileconv = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/fileconv")
                .canonicalize()
                .expect("build target/debug/fileconv first");
            let output = run(
                &SandboxConfig {
                    argv_template: vec![
                        fileconv.display().to_string(),
                        "one".into(),
                        INPUT_PLACEHOLDER.into(),
                    ],
                    limits: ResourceLimits {
                        wall_timeout: Duration::from_secs(30),
                        ..ResourceLimits::default()
                    },
                },
                SandboxInput {
                    bytes: include_bytes!(
                        "../../../../bench/markhand_web/soak/fixtures/soak-png.png"
                    )
                    .to_vec(),
                    canonical_extension: "png".into(),
                },
                &SandboxCancel::default(),
            )
            .expect("sandbox run");
            assert_eq!(
                output.exit,
                SandboxExit::Success,
                "stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("SOAK15"),
                "stdout={}",
                String::from_utf8_lossy(&output.stdout)
            );
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
}

#[cfg(unix)]
pub use imp::*;

// The production sandbox depends on Linux isolation primitives. A Windows
// worker must fail closed rather than execute conversion commands without those
// protections; this also permits the rest of the server crate to build and
// report that isolation is unavailable.
#[cfg(not(unix))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::super::limits::ResourceLimits;

    const INPUT_PLACEHOLDER: &str = "{input}";

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
            if !Path::new(&self.argv_template[0]).is_absolute() {
                return Err(SandboxError::InvalidConfig(
                    "converter executable must be an absolute path".into(),
                ));
            }
            if !self
                .argv_template
                .iter()
                .any(|arg| arg.contains(INPUT_PLACEHOLDER))
            {
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
        pub pid1_host_pid: Option<u32>,
        pub workspace_path: PathBuf,
    }

    #[derive(Debug, thiserror::Error)]
    pub enum SandboxError {
        #[error("sandbox configuration is invalid: {0}")]
        InvalidConfig(String),
        #[error("sandbox isolation is unavailable")]
        IsolationUnavailable,
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

    pub fn preflight() -> Result<(), SandboxError> {
        Err(SandboxError::IsolationUnavailable)
    }

    pub fn run(
        config: &SandboxConfig,
        _input: SandboxInput,
        _cancel: &SandboxCancel,
    ) -> Result<SandboxOutput, SandboxError> {
        config.validate()?;
        Err(SandboxError::IsolationUnavailable)
    }
}

#[cfg(not(unix))]
pub use imp::*;
