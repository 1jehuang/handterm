use anyhow::{Context, Result};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::waitpid;
use nix::unistd::Pid;
use std::ffi::{CString, OsString, c_char};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

pub type ChildEnvVar<'a> = (&'a str, &'a str);

/// Fallback search path used when `PATH` is unset, mirroring `execvp(3)`.
const DEFAULT_SEARCH_PATH: &str = "/usr/bin:/bin";

/// Everything the forked child needs in order to exec, fully prepared before
/// `forkpty` is called.
///
/// After `fork` in a multithreaded process, the child may only perform
/// async-signal-safe operations until it execs: another thread may hold the
/// allocator or environment locks at fork time, and the child's copy of such
/// a lock is never released. Calling `std::env::set_var` or allocating
/// `CString`s in the child can therefore deadlock before `exec` (observed as
/// `cargo test --workspace` hanging in the PTY tests). All allocation and
/// environment access happens here instead, in the parent.
struct PreparedExec {
    /// Candidate program paths in `execvp(3)` search order. Multiple entries
    /// exist only when the program name had to be resolved against `PATH`.
    candidates: Vec<CString>,
    /// Owned strings backing `argv`; kept alive for the raw pointer array.
    _args: Vec<CString>,
    /// Owned `KEY=VALUE` strings backing `envp`.
    _env: Vec<CString>,
    /// Null-terminated pointer array for `execve(2)`'s `argv`.
    argv: Vec<*const c_char>,
    /// Null-terminated pointer array for `execve(2)`'s `envp`.
    envp: Vec<*const c_char>,
    /// Optional directory to enter in the child before exec.
    cwd: Option<CString>,
}

impl PreparedExec {
    fn new(
        shell_path: &str,
        command: Option<&str>,
        envs: &[ChildEnvVar<'_>],
        cwd: Option<&Path>,
    ) -> Result<Self> {
        let mut args = vec![
            CString::new(shell_path)
                .with_context(|| format!("invalid shell path: {shell_path}"))?,
        ];
        if let Some(command) = command {
            args.push(CString::new("-lc").expect("valid shell flag"));
            args.push(
                CString::new(command)
                    .with_context(|| format!("invalid shell command: {command}"))?,
            );
        }

        let env_pairs = effective_env(envs);
        let candidates = program_candidates(shell_path, &env_pairs)?;
        let env: Vec<CString> = env_pairs
            .into_iter()
            .map(|(key, value)| {
                anyhow::ensure!(
                    !key.as_encoded_bytes().contains(&b'='),
                    "environment variable name contains '=': {:?}",
                    key
                );
                let mut entry = Vec::with_capacity(key.len() + value.len() + 1);
                entry.extend_from_slice(key.as_encoded_bytes());
                entry.push(b'=');
                entry.extend_from_slice(value.as_encoded_bytes());
                CString::new(entry).context("environment variable contains a NUL byte")
            })
            .collect::<Result<_>>()?;

        let argv = nul_terminated_ptrs(&args);
        let envp = nul_terminated_ptrs(&env);
        let cwd = cwd
            .map(|path| CString::new(path.as_os_str().as_encoded_bytes()))
            .transpose()
            .context("working directory contains a NUL byte")?;
        Ok(Self {
            candidates,
            _args: args,
            _env: env,
            argv,
            envp,
            cwd,
        })
    }
}

/// The environment the child should see: the parent's environment plus the
/// terminal-identifying defaults and caller-provided overrides.
fn effective_env(overrides: &[ChildEnvVar<'_>]) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    upsert_env(&mut env, "TERM", "xterm-256color");
    upsert_env(&mut env, "COLORTERM", "truecolor");
    for (key, value) in overrides {
        upsert_env(&mut env, key, value);
    }
    env
}

fn upsert_env(env: &mut Vec<(OsString, OsString)>, key: &str, value: &str) {
    let value = OsString::from(value);
    match env
        .iter_mut()
        .find(|(existing, _)| existing.as_os_str() == key)
    {
        Some(slot) => slot.1 = value,
        None => env.push((OsString::from(key), value)),
    }
}

/// Resolve the program the way `execvp(3)` would, but ahead of the fork: a
/// name containing `/` is used as-is, anything else is tried against each
/// `PATH` entry in order. The child then attempts `execve(2)` on each
/// candidate without allocating.
fn program_candidates(program: &str, env: &[(OsString, OsString)]) -> Result<Vec<CString>> {
    let invalid_path = || format!("invalid shell path: {program}");
    if program.contains('/') {
        return Ok(vec![CString::new(program).with_context(invalid_path)?]);
    }
    let search_path = env
        .iter()
        .find(|(key, _)| key.as_os_str() == "PATH")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| OsString::from(DEFAULT_SEARCH_PATH));
    let candidates: Vec<CString> = std::env::split_paths(&search_path)
        .map(|dir| {
            // An empty `PATH` entry means the current directory, per POSIX.
            let dir = if dir.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                dir
            };
            CString::new(dir.join(program).into_os_string().into_vec()).with_context(invalid_path)
        })
        .collect::<Result<_>>()?;
    anyhow::ensure!(
        !candidates.is_empty(),
        "empty PATH while resolving shell: {program}"
    );
    Ok(candidates)
}

/// Build the null-terminated `*const c_char` array `execve(2)` expects. The
/// returned pointers borrow from `strings`, which must stay alive and
/// unmodified until after the exec.
fn nul_terminated_ptrs(strings: &[CString]) -> Vec<*const c_char> {
    strings
        .iter()
        .map(|s| s.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect()
}

pub struct PtyChild {
    master_fd: OwnedFd,
    child_pid: Pid,
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        // `forkpty` makes the child a process-group leader. Explicitly hang up
        // the whole group when its terminal window closes, then reap the child
        // off the event-loop thread. Closing `master_fd` immediately after this
        // drop hook reinforces the same terminal hangup semantics.
        let _ = killpg(self.child_pid, Signal::SIGHUP);
        let child_pid = self.child_pid;
        let _ = std::thread::Builder::new()
            .name("handterm-pty-reaper".into())
            .spawn(
                move || {
                    while let Err(nix::errno::Errno::EINTR) = waitpid(child_pid, None) {}
                },
            );
    }
}

impl PtyChild {
    pub fn spawn_default_shell(columns: u16, rows: u16) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self::spawn_shell_with_env(&shell, columns, rows, &[])
    }

    pub fn spawn_default_shell_with_command_and_env(
        columns: u16,
        rows: u16,
        command: Option<&str>,
        envs: &[ChildEnvVar<'_>],
    ) -> Result<Self> {
        Self::spawn_default_shell_with_command_env_and_cwd(columns, rows, command, envs, None)
    }

    pub fn spawn_default_shell_with_command_env_and_cwd(
        columns: u16,
        rows: u16,
        command: Option<&str>,
        envs: &[ChildEnvVar<'_>],
        cwd: Option<&Path>,
    ) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        match command.filter(|command| !command.trim().is_empty()) {
            Some(command) => {
                Self::spawn_shell_inner(&shell, Some(command), columns, rows, envs, cwd)
            }
            None => Self::spawn_shell_inner(&shell, None, columns, rows, envs, cwd),
        }
    }

    pub fn spawn_shell(shell_path: &str, columns: u16, rows: u16) -> Result<Self> {
        Self::spawn_shell_with_env(shell_path, columns, rows, &[])
    }

    pub fn spawn_shell_with_env(
        shell_path: &str,
        columns: u16,
        rows: u16,
        envs: &[ChildEnvVar<'_>],
    ) -> Result<Self> {
        Self::spawn_shell_inner(shell_path, None, columns, rows, envs, None)
    }

    pub fn spawn_shell_command(
        shell_path: &str,
        command: &str,
        columns: u16,
        rows: u16,
    ) -> Result<Self> {
        Self::spawn_shell_command_with_env(shell_path, command, columns, rows, &[])
    }

    pub fn spawn_shell_command_with_env(
        shell_path: &str,
        command: &str,
        columns: u16,
        rows: u16,
        envs: &[ChildEnvVar<'_>],
    ) -> Result<Self> {
        Self::spawn_shell_inner(shell_path, Some(command), columns, rows, envs, None)
    }

    fn spawn_shell_inner(
        shell_path: &str,
        command: Option<&str>,
        columns: u16,
        rows: u16,
        envs: &[ChildEnvVar<'_>],
        cwd: Option<&Path>,
    ) -> Result<Self> {
        // Prepare argv/envp/program paths before forking: the child branch
        // below must not allocate or touch the environment (see
        // `PreparedExec`).
        let exec = PreparedExec::new(shell_path, command, envs, cwd)?;

        // Successful exec closes the writer automatically. Setup/exec failure
        // writes a compact diagnostic first, allowing the parent to distinguish
        // a launched program from a child that immediately failed to launch.
        let (error_read, error_write) = cloexec_pipe().context("exec status pipe failed")?;

        let ws = Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let result = match unsafe { forkpty(Some(&ws), None) } {
            Ok(result) => result,
            Err(error) => {
                unsafe {
                    nix::libc::close(error_read);
                    nix::libc::close(error_write);
                }
                return Err(error).context("forkpty failed");
            }
        };

        match result {
            ForkptyResult::Parent { child, master } => {
                unsafe { nix::libc::close(error_write) };
                let launch_error = read_launch_error(error_read);
                unsafe { nix::libc::close(error_read) };
                if let Some((stage, errno)) = launch_error? {
                    let _ = waitpid(child, None);
                    anyhow::bail!(
                        "child {stage} failed: {}",
                        std::io::Error::from_raw_os_error(errno)
                    );
                }
                let pty = Self {
                    master_fd: master,
                    child_pid: child,
                };
                pty.set_nonblocking()?;
                Ok(pty)
            }
            ForkptyResult::Child => {
                unsafe { nix::libc::close(error_read) };
                if let Some(cwd) = &exec.cwd {
                    // SAFETY: `cwd` is a preallocated, NUL-terminated path and
                    // `chdir(2)` is async-signal-safe after fork.
                    if unsafe { nix::libc::chdir(cwd.as_ptr()) } != 0 {
                        report_launch_error(error_write, b'c');
                        unsafe { nix::libc::_exit(126) }
                    }
                }
                // Only async-signal-safe calls are allowed here (see
                // `PreparedExec`): `execve(2)` and `_exit(2)` qualify. Try
                // each pre-resolved candidate in order, mirroring
                // `execvp(3)`.
                for program in &exec.candidates {
                    // SAFETY: `program`, `argv`, and `envp` are valid
                    // null-terminated arrays prepared before the fork; the
                    // child owns a copy of the parent's address space.
                    unsafe {
                        nix::libc::execve(program.as_ptr(), exec.argv.as_ptr(), exec.envp.as_ptr());
                    }
                }
                report_launch_error(error_write, b'e');
                unsafe { nix::libc::_exit(127) }
            }
        }
    }

    fn set_nonblocking(&self) -> Result<()> {
        let flags = fcntl(&self.master_fd, FcntlArg::F_GETFL).context("F_GETFL failed")?;
        let new_flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
        fcntl(&self.master_fd, FcntlArg::F_SETFL(new_flags)).context("F_SETFL failed")?;
        Ok(())
    }

    pub fn raw_fd(&self) -> i32 {
        self.master_fd.as_raw_fd()
    }

    #[cfg(test)]
    fn child_pid(&self) -> Pid {
        self.child_pid
    }

    pub fn write_all(&self, data: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset < data.len() {
            let n =
                nix::unistd::write(&self.master_fd, &data[offset..]).context("pty write failed")?;
            offset += n;
        }
        Ok(())
    }

    pub fn try_read(&self, out: &mut [u8]) -> Result<usize> {
        match nix::unistd::read(&self.master_fd, out) {
            Ok(n) => Ok(n),
            Err(nix::errno::Errno::EAGAIN) => Ok(0),
            Err(e) => Err(e).context("pty read failed"),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        use nix::libc::{TIOCSWINSZ, ioctl, winsize};
        use std::os::fd::AsRawFd;
        let ws = winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let ret = unsafe { ioctl(self.master_fd.as_raw_fd(), TIOCSWINSZ, &ws) };
        if ret == -1 {
            anyhow::bail!("TIOCSWINSZ failed: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }
}

fn report_launch_error(fd: i32, stage: u8) {
    let errno = nix::errno::Errno::last_raw();
    let mut message = [0_u8; 5];
    message[0] = stage;
    message[1..].copy_from_slice(&errno.to_ne_bytes());
    unsafe {
        nix::libc::write(fd, message.as_ptr().cast(), message.len());
    }
}

fn cloexec_pipe() -> std::io::Result<(i32, i32)> {
    let mut fds = [-1; 2];
    if unsafe { nix::libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for fd in fds {
        if unsafe { nix::libc::fcntl(fd, nix::libc::F_SETFD, nix::libc::FD_CLOEXEC) } == -1 {
            let error = std::io::Error::last_os_error();
            unsafe {
                nix::libc::close(fds[0]);
                nix::libc::close(fds[1]);
            }
            return Err(error);
        }
    }
    Ok((fds[0], fds[1]))
}

fn read_launch_error(fd: i32) -> Result<Option<(&'static str, i32)>> {
    let mut message = [0_u8; 5];
    let mut offset = 0;
    loop {
        let n = unsafe {
            nix::libc::read(
                fd,
                message[offset..].as_mut_ptr().cast(),
                message.len() - offset,
            )
        };
        if n == 0 {
            return if offset == 0 {
                Ok(None)
            } else {
                anyhow::bail!("child launch status pipe closed with a partial message")
            };
        }
        if n < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("reading child launch status");
        }
        offset += n as usize;
        if offset == message.len() {
            let stage = match message[0] {
                b'c' => "chdir",
                b'e' => "exec",
                _ => "setup",
            };
            return Ok(Some((
                stage,
                i32::from_ne_bytes(message[1..].try_into().unwrap()),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PtyChild;
    use std::time::{Duration, Instant};

    /// Read from the pty until `expected` appears in the output, yielding
    /// briefly between empty reads so parallel test threads are not starved.
    fn read_until_contains(child: &PtyChild, expected: &str) -> String {
        let mut buffer = [0_u8; 8192];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut output = String::new();

        while Instant::now() < deadline {
            let n = child.try_read(&mut buffer).expect("read should succeed");
            if n == 0 {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            output.push_str(&String::from_utf8_lossy(&buffer[..n]));
            if output.contains(expected) {
                return output;
            }
        }

        panic!("timed out waiting for {expected:?} in shell output: {output}");
    }

    #[test]
    fn pty_shell_echoes_text() {
        let child = PtyChild::spawn_shell("/bin/sh", 80, 24).expect("pty should spawn shell");
        child
            .write_all(b"printf 'handterm-pty-ok\\n'\\n")
            .expect("write should succeed");
        read_until_contains(&child, "handterm-pty-ok");
    }

    #[test]
    fn pty_can_start_in_an_explicit_working_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let command = "pwd";
        let child = PtyChild::spawn_default_shell_with_command_env_and_cwd(
            80,
            24,
            Some(command),
            &[],
            Some(dir.path()),
        )
        .expect("PTY should start in requested directory");

        read_until_contains(&child, &dir.path().to_string_lossy());
    }

    #[test]
    fn pty_shell_command_runs_immediately() {
        let child =
            PtyChild::spawn_shell_command("/bin/sh", "printf 'handterm-pty-command-ok\\n'", 80, 24)
                .expect("pty should spawn command shell");
        read_until_contains(&child, "handterm-pty-command-ok");
    }

    #[test]
    fn pty_child_env_matches_pre_fork_preparation() {
        // Exercises both the pre-built envp (defaults plus overrides) and the
        // execvp-style PATH resolution ("sh" has no slash).
        let child = PtyChild::spawn_shell_command_with_env(
            "sh",
            "printf 'env:%s|%s|%s\\n' \"$TERM\" \"$COLORTERM\" \"$HANDTERM_PTY_TEST\"",
            80,
            24,
            &[("HANDTERM_PTY_TEST", "fork-safe")],
        )
        .expect("pty should spawn shell resolved via PATH");
        read_until_contains(&child, "env:xterm-256color|truecolor|fork-safe");
    }

    #[test]
    fn pty_spawn_reports_exec_failure() {
        let error = PtyChild::spawn_shell("/definitely/not/a/handterm-shell", 80, 24)
            .err()
            .expect("missing executable must fail spawn");
        assert!(error.to_string().contains("child exec failed"), "{error:#}");
    }

    #[test]
    fn pty_spawn_reports_working_directory_failure() {
        let missing = std::path::Path::new("/definitely/not/a/handterm-directory");
        let error = PtyChild::spawn_default_shell_with_command_env_and_cwd(
            80,
            24,
            Some("true"),
            &[],
            Some(missing),
        )
        .err()
        .expect("missing cwd must fail spawn");
        assert!(
            error.to_string().contains("child chdir failed"),
            "{error:#}"
        );
    }

    #[test]
    fn pty_rejects_malformed_environment_names() {
        let error = PtyChild::spawn_shell_with_env("/bin/sh", 80, 24, &[("BAD=NAME", "value")])
            .err()
            .expect("malformed environment name must be rejected");
        assert!(error.to_string().contains("contains '='"), "{error:#}");
    }

    #[test]
    fn forkpty_child_is_its_own_session_and_process_group_leader() {
        let child = PtyChild::spawn_shell("/bin/sh", 80, 24).expect("pty should launch shell");
        let pid = child.child_pid().as_raw();
        assert_eq!(unsafe { nix::libc::getsid(pid) }, pid);
        assert_eq!(unsafe { nix::libc::getpgid(pid) }, pid);
    }

    #[test]
    fn dropping_pty_hangs_up_and_reaps_child() {
        let child = PtyChild::spawn_shell("/bin/sh", 80, 24).expect("pty should launch shell");
        let pid = child.child_pid();
        drop(child);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if nix::sys::signal::kill(pid, None) == Err(nix::errno::Errno::ESRCH) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("PTY child {pid} was not reaped after terminal teardown");
    }
}
