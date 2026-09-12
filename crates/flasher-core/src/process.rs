//! Structured, bounded execution of a user-selected external sunxi-fel binary.
use crate::{Cancellation, Error, assets::regular_components};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const OUTPUT_LIMIT: u64 = 64 * 1024;
#[derive(Debug)]
pub struct SunxiTool {
    path: PathBuf,
}
impl SunxiTool {
    /// Requires an explicit absolute regular executable path; never searches PATH.
    /// # Errors
    /// Rejects relative paths, symlinks, scripts on Windows, and inaccessible files.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::UnsafePath);
        }
        regular_components(path)?;
        if !fs::metadata(path)?.is_file() {
            return Err(Error::UnsafePath);
        }
        #[cfg(windows)]
        if path
            .extension()
            .is_none_or(|v| !v.eq_ignore_ascii_case("exe"))
        {
            return Err(Error::UnsafePath);
        }
        Ok(Self {
            path: fs::canonicalize(path)?,
        })
    }
    /// Executes only `--list`. This implementation cannot issue FEL writes.
    /// # Errors
    /// Reports missing permissions, failed processes, cancellation, timeout and oversized output.
    pub fn list(&self, cancel: &Cancellation) -> Result<String, Error> {
        capture(&self.path, &["--list"], Duration::from_secs(15), cancel)
    }
}
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn capture(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    cancel: &Cancellation,
) -> Result<String, Error> {
    cancel.check()?;
    let mut stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW; no interactive console.
    }
    let mut child = Running(command.spawn()?);
    let started = Instant::now();
    loop {
        cancel.check()?;
        if stdout.metadata()?.len() > OUTPUT_LIMIT || stderr.metadata()?.len() > OUTPUT_LIMIT {
            return Err(Error::Output);
        }
        if let Some(status) = child.0.try_wait()? {
            if stdout.metadata()?.len() > OUTPUT_LIMIT || stderr.metadata()?.len() > OUTPUT_LIMIT {
                return Err(Error::Output);
            }
            if !status.success() {
                return Err(Error::Process(status.code()));
            }
            return bounded_text(&mut stdout);
        }
        if started.elapsed() >= timeout {
            return Err(Error::Timeout);
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn bounded_text(file: &mut File) -> Result<String, Error> {
    use std::io::Seek;
    file.rewind()?;
    let mut data = Vec::new();
    file.take(OUTPUT_LIMIT + 1).read_to_end(&mut data)?;
    if data.len() as u64 > OUTPUT_LIMIT {
        return Err(Error::Output);
    }
    String::from_utf8(data).map_err(|_| Error::Output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str, timeout: Duration, cancel: &Cancellation) -> Result<String, Error> {
        capture(
            &std::env::current_exe()?,
            &["--exact", name, "--ignored", "--nocapture"],
            timeout,
            cancel,
        )
    }
    #[test]
    fn process_failure_is_reported() {
        assert!(matches!(
            fixture(
                "process::tests::child_failure",
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
                "process::tests::child_output",
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
                "process::tests::child_sleep",
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
            fixture("process::tests::child_sleep", Duration::from_secs(10), &c),
            Err(Error::Cancelled)
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
}
