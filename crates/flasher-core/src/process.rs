//! Structured, bounded execution of a user-selected external sunxi-fel binary.
use crate::{
    Cancellation, Error,
    assets::regular_components,
    tool::{SystemToolRunner, ToolRequest, ToolRunner},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const LIST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct SunxiTool {
    path: PathBuf,
    runner: Arc<dyn ToolRunner>,
}
impl SunxiTool {
    /// Requires an explicit absolute regular executable path; never searches PATH.
    /// # Errors
    /// Rejects relative paths, symlinks, scripts on Windows, and inaccessible files.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_with_runner(path, Arc::new(SystemToolRunner))
    }
    /// Opens the same validated path with an injected runner for scripted tests.
    /// # Errors
    /// Applies every production path check before accepting the runner.
    pub fn open_with_runner(path: &Path, runner: Arc<dyn ToolRunner>) -> Result<Self, Error> {
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
            runner,
        })
    }
    /// Executes only `--list`. This implementation cannot issue FEL writes.
    /// # Errors
    /// Reports missing permissions, failed processes, cancellation, timeout and oversized output.
    pub fn list(&self, cancel: &Cancellation) -> Result<String, Error> {
        let request = ToolRequest::new(&self.path, ["--list"]).timeout(LIST_TIMEOUT);
        self.runner
            .run(&request, cancel)
            .map(|output| output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        device::{RealFel, parse_list, select},
        tool::ToolOutput,
    };
    fn fixture(name: &str, timeout: Duration, cancel: &Cancellation) -> Result<String, Error> {
        let executable = std::env::current_exe()?;
        let request = ToolRequest::new(executable, ["--exact", name, "--ignored", "--nocapture"])
            .timeout(timeout);
        SystemToolRunner
            .run(&request, cancel)
            .map(|output| output.stdout)
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
    fn sunxi_tool_uses_only_the_injected_runner() -> Result<(), Error> {
        let executable = std::env::current_exe()?;
        let line = "USB device 001:002   Allwinner A13     01234567:89abcdef:01234567:89abcdef\n";
        let runner = crate::tool::ScriptedToolRunner::new();
        runner.expect_ok(
            ToolRequest::new(&executable, ["--list"]).timeout(Duration::from_secs(15)),
            ToolOutput {
                exit: Some(0),
                stdout: line.to_owned(),
                stderr: String::new(),
            },
        );
        let tool = SunxiTool::open_with_runner(&executable, Arc::new(runner))?;
        let device = select(&parse_list(&tool.list(&Cancellation::default())?)?)?;
        assert_eq!(device.address, 2);
        Ok(())
    }
    #[test]
    fn malformed_tool_output_is_rejected() -> Result<(), Error> {
        let executable = std::env::current_exe()?;
        let runner = crate::tool::ScriptedToolRunner::new();
        runner.expect_ok(
            ToolRequest::new(&executable, ["--list"]).timeout(Duration::from_secs(15)),
            ToolOutput {
                exit: Some(0),
                stdout: "unexpected output".into(),
                stderr: String::new(),
            },
        );
        let real = RealFel::new(SunxiTool::open_with_runner(&executable, Arc::new(runner))?);
        assert!(matches!(
            real.discover(&Cancellation::default()),
            Err(Error::Device)
        ));
        Ok(())
    }
    #[test]
    fn injected_list_failure_stops_discovery() -> Result<(), Error> {
        let executable = std::env::current_exe()?;
        let runner = crate::tool::ScriptedToolRunner::new();
        runner.expect_error(
            ToolRequest::new(&executable, ["--list"]).timeout(Duration::from_secs(15)),
            Error::Timeout,
        );
        let real = RealFel::new(SunxiTool::open_with_runner(&executable, Arc::new(runner))?);
        assert!(matches!(
            real.discover(&Cancellation::default()),
            Err(Error::Timeout)
        ));
        Ok(())
    }
    #[test]
    fn scripts_without_expectations_are_unexpected() -> Result<(), Error> {
        let executable = std::env::current_exe()?;
        let runner = Arc::new(crate::tool::ScriptedToolRunner::new());
        let tool = SunxiTool::open_with_runner(&executable, runner)?;
        assert!(matches!(
            tool.list(&Cancellation::default()),
            Err(Error::ScriptUnexpected(_))
        ));
        Ok(())
    }
}
