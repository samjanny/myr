//! Bounded subprocess I/O with a single deadline, including stdin and pipe drains.
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid subprocess output limits")]
    InvalidLimits,
    #[error("could not start subprocess")]
    Start,
    #[error("subprocess I/O failed")]
    Io,
    #[error("subprocess exceeded its deadline")]
    Timeout,
    #[error("subprocess output exceeded its limit")]
    TooLarge,
}
pub struct Output {
    pub success: bool,
    /// None means termination without a normal exit code; never assume zero.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct OutputLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub combined_bytes: usize,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            stdout_bytes: OUTPUT_LIMIT,
            stderr_bytes: OUTPUT_LIMIT,
            combined_bytes: OUTPUT_LIMIT * 2,
        }
    }
}

/// Keep normal OS/auth-directory discovery, but no API credentials, cloud routing,
/// preload hooks, inherited agent session, or arbitrary project environment.
pub fn clean_command(executable: &std::path::Path) -> Command {
    let mut command = Command::new(executable);
    command.env_clear();
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "USERPROFILE",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

fn terminate(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Some(root) = std::env::var_os("SystemRoot") {
            let _ = Command::new(std::path::Path::new(&root).join("System32/taskkill.exe"))
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn run(command: Command, input: Vec<u8>, timeout: Duration) -> Result<Output, Error> {
    run_bounded(command, input, timeout, OutputLimits::default())
}

/// Shared transport/verifier primitive. Process limits and environment selection
/// do not establish a filesystem or network sandbox. On failure no successful
/// execution record is returned; callers must not infer an exit code.
pub fn run_bounded(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
    limits: OutputLimits,
) -> Result<Output, Error> {
    if [
        limits.stdout_bytes,
        limits.stderr_bytes,
        limits.combined_bytes,
    ]
    .iter()
    .any(|n| *n == 0 || *n > 64 * 1024 * 1024)
    {
        return Err(Error::InvalidLimits);
    }
    if timeout.is_zero() {
        return Err(Error::Timeout);
    }
    let deadline = Instant::now().checked_add(timeout).ok_or(Error::Timeout)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| Error::Start)?;
    let mut stdin = child.stdin.take().ok_or(Error::Io)?;
    let stdout = child.stdout.take().ok_or(Error::Io)?;
    let stderr = child.stderr.take().ok_or(Error::Io)?;
    let (tx, rx) = mpsc::channel();
    let writer = tx.clone();
    std::thread::spawn(move || {
        let result = stdin
            .write_all(&input)
            .map(|_| Vec::new())
            .map_err(|_| Error::Io);
        drop(stdin);
        let _ = writer.send((0, result));
    });
    let total = Arc::new(AtomicUsize::new(0));
    for (index, mut pipe, limit) in [
        (
            1,
            Box::new(stdout) as Box<dyn Read + Send>,
            limits.stdout_bytes.min(limits.combined_bytes),
        ),
        (
            2,
            Box::new(stderr) as Box<dyn Read + Send>,
            limits.stderr_bytes.min(limits.combined_bytes),
        ),
    ] {
        let sender = tx.clone();
        let total = Arc::clone(&total);
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = (|| {
                let mut buffer = [0; 8192];
                loop {
                    let count = pipe.read(&mut buffer).map_err(|_| Error::Io)?;
                    if count == 0 {
                        return Ok(bytes);
                    }
                    let previous = total.fetch_add(count, Ordering::Relaxed);
                    if bytes.len() + count > limit || previous + count > limits.combined_bytes {
                        return Err(Error::TooLarge);
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
            })();
            let _ = sender.send((index, result));
        });
    }
    drop(tx);
    let mut streams: [Option<Vec<u8>>; 3] = [None, None, None];
    let result = (|| {
        while streams.iter().any(Option::is_none) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(Error::Timeout)?;
            let (index, result) = rx.recv_timeout(remaining).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                _ => Error::Io,
            })?;
            streams[index] = Some(result?);
            let captured: usize = streams[1..].iter().flatten().map(Vec::len).sum();
            if captured > limits.combined_bytes {
                return Err(Error::TooLarge);
            }
        }
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            if let Some(status) = child.try_wait().map_err(|_| Error::Io)? {
                return Ok(Output {
                    success: status.success(),
                    exit_code: status.code(),
                    stdout: streams[1].take().unwrap(),
                    stderr: streams[2].take().unwrap(),
                });
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        terminate(&mut child);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subprocess_fixture() {
        match std::env::var("MYR_TEST_PROCESS").as_deref() {
            Ok("echo") => {
                let mut input = String::new();
                std::io::stdin().read_to_string(&mut input).unwrap();
                println!("fixture:{input}");
                eprintln!("fixture-stderr");
                std::process::exit(0);
            }
            Ok("large") => {
                let _ = std::io::stdout().write_all(&vec![b'x'; OUTPUT_LIMIT + 1024]);
                std::process::exit(0);
            }
            Ok("sleep") => {
                std::thread::sleep(Duration::from_secs(30));
                std::process::exit(0);
            }
            Ok("failure") => {
                eprintln!("fixture failure diagnostic");
                std::process::exit(23);
            }
            Ok("split_then_sleep") => {
                std::io::stdout().write_all(&[b'x'; 100]).unwrap();
                std::io::stdout().flush().unwrap();
                std::io::stderr().write_all(&[b'y'; 100]).unwrap();
                std::io::stderr().flush().unwrap();
                std::thread::sleep(Duration::from_secs(30));
                std::process::exit(0);
            }
            _ => {}
        }
    }
    fn fixture(mode: &str) -> Command {
        let mut c = clean_command(&std::env::current_exe().unwrap());
        c.args([
            "--exact",
            "process::tests::subprocess_fixture",
            "--nocapture",
        ])
        .env("MYR_TEST_PROCESS", mode);
        c
    }
    #[test]
    fn subprocess_deadline_output_limit_and_pipe_capture() {
        let output = run(fixture("echo"), b"input".to_vec(), Duration::from_secs(5)).unwrap();
        assert!(output.success);
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("fixture:input")
        );
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("fixture-stderr")
        );
        assert!(matches!(
            run(fixture("large"), vec![], Duration::from_secs(5)),
            Err(Error::TooLarge)
        ));
        assert!(matches!(
            run(fixture("sleep"), vec![], Duration::from_millis(200)),
            Err(Error::Timeout)
        ));
    }
    #[test]
    fn configurable_limits_and_nonzero_exit_are_preserved() {
        assert!(matches!(
            run_bounded(
                fixture("split_then_sleep"),
                vec![],
                Duration::from_secs(2),
                OutputLimits {
                    stdout_bytes: 4096,
                    stderr_bytes: 4096,
                    combined_bytes: 150,
                }
            ),
            Err(Error::TooLarge)
        ));
        let output = run(fixture("failure"), vec![], Duration::from_secs(5)).unwrap();
        assert!(!output.success);
        assert_eq!(output.exit_code, Some(23));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("failure diagnostic")
        );
        let limits = OutputLimits {
            stdout_bytes: OUTPUT_LIMIT * 2,
            stderr_bytes: 4096,
            combined_bytes: OUTPUT_LIMIT * 2,
        };
        let output = run_bounded(fixture("large"), vec![], Duration::from_secs(5), limits).unwrap();
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.len() > OUTPUT_LIMIT);
        let output = run(fixture("echo"), vec![], Duration::from_secs(5)).unwrap();
        let limits = OutputLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            combined_bytes: output.stdout.len() + output.stderr.len() - 1,
        };
        assert!(matches!(
            run_bounded(fixture("echo"), vec![], Duration::from_secs(5), limits),
            Err(Error::TooLarge)
        ));
        assert!(matches!(
            run_bounded(
                fixture("echo"),
                vec![],
                Duration::from_secs(5),
                OutputLimits {
                    combined_bytes: 0,
                    ..limits
                }
            ),
            Err(Error::InvalidLimits)
        ));
    }

    #[test]
    fn environment_is_allowlisted_and_contains_no_api_routing() {
        let command = clean_command(std::path::Path::new("fixture"));
        for (name, _) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                !name.contains("API_KEY")
                    && !name.contains("AUTH_TOKEN")
                    && !name.contains("USE_BEDROCK")
                    && !name.contains("BASE_URL")
                    && name != "NODE_OPTIONS"
            );
        }
    }
}
