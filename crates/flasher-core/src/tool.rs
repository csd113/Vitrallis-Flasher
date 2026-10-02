//! Narrow external-process boundary for recovery orchestration.
//!
//! Production code hands a [`ToolRequest`] to a [`ToolRunner`]; only
//! [`SystemToolRunner`] starts a real child process. Tests use
//! [`ScriptedToolRunner`] to script exact argv/environment/stdin, failures and
//! timeouts deterministically.
use crate::{Cancellation, Error, script::Scripted};
use std::{
    fmt::Debug,
    fs::File,
    io::{Read, Seek, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

/// Maximum captured stdout/stderr and forwarded stdin per invocation.
pub const OUTPUT_LIMIT: u64 = 64 * 1024;

/// One fully specified tool invocation. No shell interpretation is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRequest {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub stdin: Vec<u8>,
    pub timeout: Duration,
    pub output_limit: u64,
}
impl ToolRequest {
    #[must_use]
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            environment: Vec::new(),
            stdin: Vec::new(),
            timeout: Duration::from_secs(15),
            output_limit: OUTPUT_LIMIT,
        }
    }
    #[must_use]
    pub fn environment(
        mut self,
        variables: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.environment = variables
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }
    #[must_use]
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = bytes.into();
        self
    }
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    #[must_use]
    pub const fn output_limit(mut self, limit: u64) -> Self {
        self.output_limit = limit;
        self
    }
}

/// A completed invocation. Non-zero exits are errors, not outputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutput {
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Executes one bounded, cancellable external command.
pub trait ToolRunner: Send + Sync + Debug {
    /// # Errors
    /// Returns cancellation, timeout, spawn, exit-status, bounds or I/O errors.
    fn run(&self, request: &ToolRequest, cancel: &Cancellation) -> Result<ToolOutput, Error>;
}

/// The only production runner: a real child process with bounded output.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemToolRunner;
impl ToolRunner for SystemToolRunner {
    fn run(&self, request: &ToolRequest, cancel: &Cancellation) -> Result<ToolOutput, Error> {
        capture(request, cancel)
    }
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn capture(request: &ToolRequest, cancel: &Cancellation) -> Result<ToolOutput, Error> {
    cancel.check()?;
    if request.stdin.len() as u64 > request.output_limit {
        return Err(Error::Output);
    }
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .envs(request.environment.iter().map(|(key, value)| (key, value)))
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    if request.stdin.is_empty() {
        command.stdin(Stdio::null());
    } else {
        command.stdin(Stdio::piped());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW; no interactive console.
    }
    let mut child = Running(command.spawn()?);
    if !request.stdin.is_empty()
        && let Some(mut pipe) = child.0.stdin.take()
    {
        pipe.write_all(&request.stdin)?;
    }
    let started = Instant::now();
    loop {
        cancel.check()?;
        if over_limit(&stdout, &stderr, request.output_limit)? {
            return Err(Error::Output);
        }
        if let Some(status) = child.0.try_wait()? {
            if over_limit(&stdout, &stderr, request.output_limit)? {
                return Err(Error::Output);
            }
            if !status.success() {
                return Err(Error::Process(status.code()));
            }
            return Ok(ToolOutput {
                exit: status.code(),
                stdout: bounded_text(&mut stdout, request.output_limit)?,
                stderr: bounded_text(&mut stderr, request.output_limit)?,
            });
        }
        if started.elapsed() >= request.timeout {
            return Err(Error::Timeout);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn over_limit(stdout: &File, stderr: &File, limit: u64) -> Result<bool, Error> {
    Ok(stdout.metadata()?.len() > limit || stderr.metadata()?.len() > limit)
}

fn bounded_text(file: &mut File, limit: u64) -> Result<String, Error> {
    file.rewind()?;
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(Error::Output);
    }
    String::from_utf8(data).map_err(|_| Error::Output)
}

/// Deterministic runner for tests: ordered expectations, recorded calls and
/// terminal failure injection.
#[derive(Debug)]
pub struct ScriptedToolRunner {
    script: Mutex<Scripted<ToolRequest, ToolOutput>>,
}
impl ScriptedToolRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            script: Mutex::new(Scripted::new()),
        }
    }
    /// Registers the next expected request and successful output.
    pub fn expect_ok(&self, request: ToolRequest, output: ToolOutput) {
        self.lock().expect_ok(request, output);
    }
    /// Registers the next expected request and an injected failure.
    pub fn expect_error(&self, request: ToolRequest, error: Error) {
        self.lock().expect_err(request, error);
    }
    #[must_use]
    pub fn calls(&self) -> Vec<ToolRequest> {
        self.lock().calls().to_vec()
    }
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.lock().remaining()
    }
    fn lock(&self) -> MutexGuard<'_, Scripted<ToolRequest, ToolOutput>> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
impl Default for ScriptedToolRunner {
    fn default() -> Self {
        Self::new()
    }
}
impl ToolRunner for ScriptedToolRunner {
    fn run(&self, request: &ToolRequest, cancel: &Cancellation) -> Result<ToolOutput, Error> {
        if let Err(cancelled) = cancel.check() {
            self.lock().poison();
            return Err(cancelled);
        }
        self.lock().call(request.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str, timeout: Duration, cancel: &Cancellation) -> Result<ToolOutput, Error> {
        let executable = std::env::current_exe()?;
        let request = ToolRequest::new(executable, ["--exact", name, "--ignored", "--nocapture"])
            .timeout(timeout);
        SystemToolRunner.run(&request, cancel)
    }
    #[test]
    fn process_failure_is_reported() {
        assert!(matches!(
            fixture(
                "tool::tests::child_failure",
                Duration::from_secs(10),
                &Cancellation::default()
            ),
            Err(Error::Process(_))
        ));
    }
    #[test]
    fn oversized_process_output_is_rejected() {
        assert!(matches!(
            fixture(
                "tool::tests::child_output",
                Duration::from_secs(10),
                &Cancellation::default()
            ),
            Err(Error::Output)
        ));
    }
    #[test]
    fn timeout_kills_and_reaps_child() {
        assert!(matches!(
            fixture(
                "tool::tests::child_sleep",
                Duration::from_millis(30),
                &Cancellation::default()
            ),
            Err(Error::Timeout)
        ));
    }
    #[test]
    fn cancelled_process_never_starts() {
        let c = Cancellation::default();
        c.cancel();
        assert!(matches!(
            fixture("tool::tests::child_sleep", Duration::from_secs(10), &c),
            Err(Error::Cancelled)
        ));
    }
    #[test]
    fn spawn_failure_is_reported() -> Result<(), Error> {
        let directory = crate::assets::temporary_directory()?;
        let missing = directory.path().join("missing-tool");
        let request = ToolRequest::new(&missing, std::iter::empty::<&str>());
        assert!(matches!(
            SystemToolRunner.run(&request, &Cancellation::default()),
            Err(Error::Io(_))
        ));
        Ok(())
    }
    #[test]
    fn exit_status_and_stderr_are_captured() -> Result<(), Error> {
        let executable = std::env::current_exe()?;
        let request = ToolRequest::new(
            executable,
            [
                "--exact",
                "tool::tests::child_stderr",
                "--ignored",
                "--nocapture",
            ],
        );
        let output = SystemToolRunner.run(&request, &Cancellation::default())?;
        assert_eq!(output.exit, Some(0));
        assert!(output.stderr.contains("fixture-stderr"));
        Ok(())
    }
    #[test]
    fn non_zero_exit_code_is_reported() {
        assert!(matches!(
            fixture(
                "tool::tests::child_exit_code",
                Duration::from_secs(10),
                &Cancellation::default()
            ),
            Err(Error::Process(Some(7)))
        ));
    }
    #[test]
    fn stdin_and_environment_are_forwarded() -> Result<(), Error> {
        let executable = std::env::current_exe()?;
        let request = ToolRequest::new(
            executable,
            [
                "--exact",
                "tool::tests::child_echo_stdin",
                "--ignored",
                "--nocapture",
            ],
        )
        .stdin(b"from-stdin".to_vec())
        .environment([("VITRALLIS_TOOL_FIXTURE", "from-env")]);
        let output = SystemToolRunner.run(&request, &Cancellation::default())?;
        assert!(output.stdout.contains("stdin=from-stdin"));
        assert!(output.stdout.contains("env=from-env"));
        Ok(())
    }
    #[test]
    fn scripted_runner_refuses_calls_after_cancellation() {
        let runner = ScriptedToolRunner::new();
        runner.expect_ok(
            ToolRequest::new("/tool", ["--list"]),
            ToolOutput {
                exit: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        let request = ToolRequest::new("/tool", ["--list"]);
        assert!(matches!(
            runner.run(&request, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(runner.remaining(), 1);
        assert!(matches!(
            runner.run(&request, &Cancellation::default()),
            Err(Error::ScriptUnexpected(_))
        ));
    }
    #[test]
    fn scripted_runner_reports_wrong_arguments() {
        let runner = ScriptedToolRunner::new();
        runner.expect_ok(
            ToolRequest::new("/tool", ["--list"]),
            ToolOutput {
                exit: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let other = ToolRequest::new("/tool", ["--flash"]);
        assert!(matches!(
            runner.run(&other, &Cancellation::default()),
            Err(Error::ScriptMismatch { .. })
        ));
    }
    #[test]
    #[ignore = "executed by the process lifecycle test"]
    fn child_failure() {
        panic!("deliberate child fixture failure");
    }
    #[test]
    #[ignore = "executed by the output bound test"]
    fn child_output() {
        println!("{}", "x".repeat(65_537));
    }
    #[test]
    #[ignore = "executed by the timeout test"]
    fn child_sleep() {
        thread::sleep(Duration::from_secs(2));
    }
    #[test]
    #[ignore = "executed by the stderr capture test"]
    fn child_stderr() {
        eprintln!("fixture-stderr");
    }
    #[test]
    #[ignore = "executed by the exit-code test"]
    fn child_exit_code() {
        std::process::exit(7);
    }
    #[test]
    #[ignore = "executed by the environment/stdin test"]
    fn child_echo_stdin() {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .expect("fixture stdin");
        println!(
            "stdin={} env={}",
            input,
            std::env::var("VITRALLIS_TOOL_FIXTURE").unwrap_or_default()
        );
    }
}
