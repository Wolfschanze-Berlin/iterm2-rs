//! PTY management: spawn a shell via ConPTY and read/write the PTY.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

/// Manages a PTY connection to a shell process.
pub struct PtyHandle {
    master_writer: Box<dyn Write + Send>,
    master_pty: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    reader_thread: Option<thread::JoinHandle<()>>,
}

impl PtyHandle {
    /// Spawn the best available shell with the given size.
    ///
    /// Detection order on Windows: pwsh.exe > powershell.exe > cmd.exe.
    pub fn spawn(cols: u16, rows: u16) -> anyhow::Result<Self> {
        let (shell, args) = detect_shell();
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        Self::spawn_command(&shell, &arg_refs, cols, rows)
    }

    /// Spawn a specific command inside a PTY.
    pub fn spawn_command(cmd: &str, args: &[&str], cols: u16, rows: u16) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut command = CommandBuilder::new(cmd);
        command.args(args);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // Tell PowerShell's PSReadLine we support VT input sequences.
        command.env("ITERM2_RS", "1");

        let child = pair.slave.spawn_command(command)?;
        // Drop the slave side; the master side owns the PTY now.
        drop(pair.slave);

        let master_writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        log::debug!("PTY reader error: {e}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            master_writer,
            master_pty: pair.master,
            child,
            output_rx: rx,
            reader_thread: Some(reader_thread),
        })
    }

    /// Write bytes to the PTY (keyboard input).
    pub fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.master_writer.write_all(data)?;
        self.master_writer.flush()?;
        Ok(())
    }

    /// Try to receive output bytes (non-blocking).
    pub fn try_recv(&self) -> Option<Vec<u8>> {
        self.output_rx.try_recv().ok()
    }

    /// Resize the PTY.
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master_pty.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Check if the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        // Graceful shutdown: drop the writer first to send EOF to the shell,
        // then wait briefly for the child to exit on its own before killing.
        // This avoids ConPTY cleanup crashes (0xcfffffff) on Windows.

        // Drop the writer to close the stdin pipe.
        // (We can't drop master_writer directly since it's not Option,
        //  but we can close it by writing an EOF / Ctrl+D.)
        let _ = self.master_writer.write_all(&[0x04]); // Ctrl+D (EOF)
        let _ = self.master_writer.flush();

        // Give the shell a moment to exit gracefully.
        for _ in 0..10 {
            match self.child.try_wait() {
                Ok(Some(_)) => break, // Child exited cleanly.
                _ => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }

        // Force kill only if still running.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }

        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Detect the best available shell on Windows.
///
/// Checks for PowerShell 7 (pwsh.exe), then PowerShell 5.1 (powershell.exe),
/// then falls back to cmd.exe via the COMSPEC environment variable.
fn detect_shell() -> (String, Vec<String>) {
    if shell_exists("pwsh.exe") {
        return ("pwsh.exe".into(), vec!["-NoLogo".into()]);
    }
    if shell_exists("powershell.exe") {
        return ("powershell.exe".into(), vec!["-NoLogo".into()]);
    }
    let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    (comspec, vec![])
}

/// Check whether a given executable can be found on the system PATH
/// by invoking `where.exe` (the Windows equivalent of `which`).
fn shell_exists(name: &str) -> bool {
    std::process::Command::new("where.exe")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
