use anyhow::{Context, Result};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::unistd::{Pid, execvp};
use std::ffi::{CStr, CString};
use std::os::fd::OwnedFd;

pub struct PtyChild {
    master_fd: OwnedFd,
    _child_pid: Pid,
}

impl PtyChild {
    pub fn spawn_default_shell(columns: u16, rows: u16) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self::spawn_shell(&shell, columns, rows)
    }

    pub fn spawn_shell(shell_path: &str, columns: u16, rows: u16) -> Result<Self> {
        let ws = Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: This is called before any application threads are started.
        // The child does an immediate execvp and does not return to Rust code.
        let result = unsafe { forkpty(Some(&ws), None) }.context("forkpty failed")?;

        match result {
            ForkptyResult::Parent { child, master } => Ok(Self {
                master_fd: master,
                _child_pid: child,
            }),
            ForkptyResult::Child => {
                let shell = CString::new(shell_path)
                    .with_context(|| format!("invalid shell path: {shell_path}"))?;
                let args: [&CStr; 1] = [shell.as_c_str()];
                let _ = execvp(shell.as_c_str(), &args);

                // Child must not unwind into parent runtime.
                std::process::exit(127);
            }
        }
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
        nix::unistd::read(&self.master_fd, out).context("pty read failed")
    }
}

#[cfg(test)]
mod tests {
    use super::PtyChild;
    use std::time::{Duration, Instant};

    #[test]
    fn pty_shell_echoes_text() {
        let child = PtyChild::spawn_shell("/bin/sh", 80, 24).expect("pty should spawn shell");
        child
            .write_all(b"printf 'handterm-pty-ok\\n'\\n")
            .expect("write should succeed");

        let mut buffer = [0_u8; 8192];
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut output = String::new();

        while Instant::now() < deadline {
            let n = child.try_read(&mut buffer).expect("read should succeed");
            if n == 0 {
                continue;
            }
            output.push_str(&String::from_utf8_lossy(&buffer[..n]));
            if output.contains("handterm-pty-ok") {
                return;
            }
        }

        panic!("timed out waiting for shell output: {output}");
    }
}
