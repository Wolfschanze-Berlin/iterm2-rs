// =============================================================================
// Spike: Test tmux.exe -CC via ConPTY on Windows
// GitHub Issue #3
//
// Goal: Determine whether tmux in control mode (-CC) works when spawned
//       through Windows ConPTY (via the portable-pty crate).
//
// What we're looking for:
//   - Can we spawn tmux.exe -CC through ConPTY at all?
//   - Does tmux emit control-mode protocol markers (%begin, %end, %output, %exit)?
//   - Can we send commands (like "list-windows") via stdin and get responses?
//   - As a fallback, does WSL tmux -CC work through ConPTY?
//
// tmux control mode protocol reference:
//   When tmux starts in -CC mode, it prints a greeting like:
//     %begin <time> <flags>
//     ...initial output...
//     %end <time> <flags>
//   Subsequent notifications use %output, %exit, %session-changed, etc.
// =============================================================================

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};
use std::sync::mpsc;
use std::thread;

/// How long to wait for output from tmux before giving up.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Known tmux control-mode protocol markers.
const MARKERS: &[&str] = &["%begin", "%end", "%output", "%exit", "%session-changed",
                            "%session-renamed", "%window-add", "%window-close",
                            "%layout-change", "%sessions-changed"];

/// Result of a single tmux spawn test.
#[derive(Debug)]
struct SpawnResult {
    spawn_ok: bool,
    lines: Vec<String>,
    markers_found: Vec<String>,
    control_mode_detected: bool,
    command_response: bool,
    error: Option<String>,
}

impl Default for SpawnResult {
    fn default() -> Self {
        Self {
            spawn_ok: false,
            lines: Vec::new(),
            markers_found: Vec::new(),
            control_mode_detected: false,
            command_response: false,
            error: None,
        }
    }
}

impl SpawnResult {
    fn verdict(&self) -> &'static str {
        if !self.spawn_ok {
            "NOT VIABLE"
        } else if self.control_mode_detected && self.command_response {
            "VIABLE"
        } else if self.control_mode_detected {
            "NEEDS WORKAROUND"
        } else if self.spawn_ok && !self.lines.is_empty() {
            "NEEDS WORKAROUND"
        } else {
            "NOT VIABLE"
        }
    }
}

/// Try to spawn tmux via ConPTY and test control mode.
fn test_conpty_tmux(label: &str, program: &str, args: &[&str]) -> SpawnResult {
    let mut result = SpawnResult::default();

    println!("\n--- Testing: {} ---", label);
    println!("  Command: {} {}", program, args.join(" "));

    // 1. Open a ConPTY pair
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Failed to open pty: {}", e);
            println!("  [FAIL] {}", msg);
            result.error = Some(msg);
            return result;
        }
    };

    // 2. Build the command
    let mut cmd = CommandBuilder::new(program);
    for arg in args {
        cmd.arg(arg);
    }

    // 3. Spawn the child
    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => {
            println!("  [OK] Process spawned");
            result.spawn_ok = true;
            c
        }
        Err(e) => {
            let msg = format!("Failed to spawn: {}", e);
            println!("  [FAIL] {}", msg);
            result.error = Some(msg);
            return result;
        }
    };

    // Drop slave so we don't hold the other end open unnecessarily
    drop(pair.slave);

    // 4. Get reader and writer
    let reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Failed to clone reader: {}", e);
            println!("  [FAIL] {}", msg);
            result.error = Some(msg);
            return result;
        }
    };

    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            let msg = format!("Failed to take writer: {}", e);
            println!("  [FAIL] {}", msg);
            result.error = Some(msg);
            return result;
        }
    };

    // 5. Read stdout in a background thread (BufReader line-by-line)
    let (tx, rx) = mpsc::channel::<String>();
    let reader_thread = thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let start = Instant::now();

    // 6. Wait a moment for tmux to start, then send a command
    thread::sleep(Duration::from_secs(2));

    println!("  Sending 'list-windows' command...");
    let send_ok = writer.write_all(b"list-windows\n").is_ok() && writer.flush().is_ok();
    if send_ok {
        println!("  [OK] Command sent");
    } else {
        println!("  [WARN] Failed to send command");
    }

    // 7. Collect output until timeout
    println!("  Reading output (timeout {}s)...", READ_TIMEOUT.as_secs());
    while start.elapsed() < READ_TIMEOUT {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let ts = start.elapsed().as_secs_f32();
                println!("  [{:6.2}s] {}", ts, line);
                result.lines.push(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                println!("  (reader disconnected)");
                break;
            }
        }
    }

    // 8. Analyze output for markers
    for line in &result.lines {
        for marker in MARKERS {
            if line.contains(marker) && !result.markers_found.contains(&marker.to_string()) {
                result.markers_found.push(marker.to_string());
            }
        }
    }

    result.control_mode_detected = !result.markers_found.is_empty();

    // Check if we got any response after sending list-windows.
    // A response would typically appear between %begin and %end markers,
    // or contain window information.
    result.command_response = result.lines.iter().any(|l| {
        l.contains("list-windows") || l.contains("%begin") || l.contains("window")
    });

    // 9. Try to kill the child
    println!("  Killing child process...");
    let _ = child.kill();
    let _ = child.wait();
    drop(writer);
    let _ = reader_thread.join();

    println!("  Total lines captured: {}", result.lines.len());
    println!("  Markers found: {:?}", result.markers_found);

    result
}

/// Fallback: test with std::process::Command (no ConPTY, raw pipes)
fn test_raw_pipes(label: &str, program: &str, args: &[&str]) -> SpawnResult {
    let mut result = SpawnResult::default();

    println!("\n--- Testing (raw pipes): {} ---", label);
    println!("  Command: {} {}", program, args.join(" "));

    let mut child = match std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => {
            println!("  [OK] Process spawned (raw pipes)");
            result.spawn_ok = true;
            c
        }
        Err(e) => {
            let msg = format!("Failed to spawn: {}", e);
            println!("  [FAIL] {}", msg);
            result.error = Some(msg);
            return result;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();

    let (tx, rx) = mpsc::channel::<String>();
    let reader_thread = thread::spawn(move || {
        let buf = BufReader::new(stdout);
        for line in buf.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let start = Instant::now();
    thread::sleep(Duration::from_secs(2));

    println!("  Sending 'list-windows' command...");
    let _ = stdin.write_all(b"list-windows\n");
    let _ = stdin.flush();

    println!("  Reading output (timeout {}s)...", READ_TIMEOUT.as_secs());
    while start.elapsed() < READ_TIMEOUT {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let ts = start.elapsed().as_secs_f32();
                println!("  [{:6.2}s] {}", ts, line);
                result.lines.push(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                println!("  (reader disconnected)");
                break;
            }
        }
    }

    for line in &result.lines {
        for marker in MARKERS {
            if line.contains(marker) && !result.markers_found.contains(&marker.to_string()) {
                result.markers_found.push(marker.to_string());
            }
        }
    }

    result.control_mode_detected = !result.markers_found.is_empty();
    result.command_response = result.lines.iter().any(|l| {
        l.contains("list-windows") || l.contains("%begin") || l.contains("window")
    });

    let _ = child.kill();
    let _ = child.wait();
    drop(stdin);
    let _ = reader_thread.join();

    println!("  Total lines captured: {}", result.lines.len());
    println!("  Markers found: {:?}", result.markers_found);

    result
}

fn print_result(_label: &str, r: &SpawnResult) {
    println!("  ConPTY spawn: {}", if r.spawn_ok { "OK" } else { "FAIL" });
    println!("  Control mode detected: {}", if r.control_mode_detected { "YES" } else { "NO" });
    if !r.markers_found.is_empty() {
        println!("  Protocol markers found: {:?}", r.markers_found);
    }
    println!("  Command response received: {}", if r.command_response { "YES" } else { "NO" });
    if let Some(ref e) = r.error {
        println!("  Error: {}", e);
    }
    println!("  Verdict: {}", r.verdict());
}

fn main() {
    println!("=== tmux.exe -CC ConPTY Spike ===");
    println!("Testing whether tmux control mode works via ConPTY on Windows\n");

    let tmux_exe = r"C:\Users\franc\AppData\Local\Microsoft\WinGet\Links\tmux.exe";

    // -------------------------------------------------------------------------
    // Test 1: tmux.exe -CC via ConPTY
    // -------------------------------------------------------------------------
    let conpty_result = test_conpty_tmux(
        "tmux.exe -CC via ConPTY",
        tmux_exe,
        &["-CC", "new-session"],
    );

    // -------------------------------------------------------------------------
    // Test 2: tmux.exe -CC via raw pipes (fallback / comparison)
    // -------------------------------------------------------------------------
    let raw_result = test_raw_pipes(
        "tmux.exe -CC via raw pipes",
        tmux_exe,
        &["-CC", "new-session"],
    );

    // -------------------------------------------------------------------------
    // Test 3: WSL tmux -CC via ConPTY (if WSL is available)
    // -------------------------------------------------------------------------
    let wsl_conpty_result = test_conpty_tmux(
        "WSL tmux -CC via ConPTY",
        "wsl.exe",
        &["tmux", "-CC", "new-session"],
    );

    // -------------------------------------------------------------------------
    // Test 4: WSL tmux -CC via raw pipes (fallback)
    // -------------------------------------------------------------------------
    let wsl_raw_result = test_raw_pipes(
        "WSL tmux -CC via raw pipes",
        "wsl.exe",
        &["tmux", "-CC", "new-session"],
    );

    // -------------------------------------------------------------------------
    // Summary
    // -------------------------------------------------------------------------
    println!("\n\n=== tmux.exe -CC ConPTY Spike Results ===\n");

    println!("Windows tmux.exe (ConPTY):");
    print_result("tmux.exe ConPTY", &conpty_result);

    println!("\nWindows tmux.exe (raw pipes):");
    print_result("tmux.exe raw pipes", &raw_result);

    println!("\nWSL tmux (ConPTY):");
    print_result("WSL ConPTY", &wsl_conpty_result);

    println!("\nWSL tmux (raw pipes):");
    print_result("WSL raw pipes", &wsl_raw_result);

    // Overall recommendation
    println!("\n--- Overall Recommendation ---");
    let all = [
        ("tmux.exe + ConPTY", &conpty_result),
        ("tmux.exe + raw pipes", &raw_result),
        ("WSL tmux + ConPTY", &wsl_conpty_result),
        ("WSL tmux + raw pipes", &wsl_raw_result),
    ];

    let viable: Vec<_> = all.iter().filter(|(_, r)| r.verdict() == "VIABLE").collect();
    let workaround: Vec<_> = all.iter().filter(|(_, r)| r.verdict() == "NEEDS WORKAROUND").collect();

    if !viable.is_empty() {
        println!("VIABLE approaches:");
        for (name, _) in &viable {
            println!("  - {}", name);
        }
    }
    if !workaround.is_empty() {
        println!("Approaches needing workarounds:");
        for (name, _) in &workaround {
            println!("  - {}", name);
        }
    }
    if viable.is_empty() && workaround.is_empty() {
        println!("No viable approach found. tmux -CC on Windows may not work.");
        println!("Consider: custom tmux protocol parser, or SSH-based approach.");
    }
}
