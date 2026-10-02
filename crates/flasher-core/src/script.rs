//! Reusable ordered script harness for deterministic seam tests.
//!
//! A script is a list of expected calls, each bound to a scripted result. Every
//! attempt is recorded. A mismatch or an unexpected call fails the whole script
//! and every later call is rejected. A scripted error is terminal by default so
//! tests can assert that no later operation ran after a failure; call
//! [`Scripted::resume`] only when a test deliberately wants to script a retry.
use crate::Error;
use std::{collections::VecDeque, fmt::Debug};

/// Typed, ordered expectations for one seam.
#[derive(Debug)]
pub struct Scripted<C, R> {
    pending: VecDeque<(C, Result<R, Error>)>,
    calls: Vec<C>,
    failed: bool,
}

impl<C, R> Default for Scripted<C, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, R> Scripted<C, R> {
    /// Creates an empty script; every call is unexpected until one is added.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            calls: Vec::new(),
            failed: false,
        }
    }
    /// Adds the next expected call and its scripted result.
    pub fn expect(&mut self, call: C, result: Result<R, Error>) {
        self.pending.push_back((call, result));
    }
    /// Adds the next expected call with a successful reply.
    pub fn expect_ok(&mut self, call: C, reply: R) {
        self.expect(call, Ok(reply));
    }
    /// Adds the next expected call with an injected failure.
    pub fn expect_err(&mut self, call: C, error: Error) {
        self.expect(call, Err(error));
    }
    /// Every attempted call, including calls rejected by the script.
    #[must_use]
    pub fn calls(&self) -> &[C] {
        &self.calls
    }
    /// Expectations that have not been consumed yet.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.pending.len()
    }
    /// True when every expectation has been consumed in order.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }
    /// True once the script has failed and later calls are refused.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.failed
    }
    /// Fails the script without producing a result, for example after
    /// cancellation was observed before a seam call.
    pub const fn poison(&mut self) {
        self.failed = true;
    }
    /// Clears a failure, letting a test deliberately script a retry.
    pub const fn resume(&mut self) {
        self.failed = false;
    }
    /// Consumes the next expectation, or fails the script.
    ///
    /// # Errors
    /// Returns `ScriptUnexpected` for an unexpected, extra or post-failure
    /// call, and `ScriptMismatch` when arguments differ from the expectation.
    pub fn call(&mut self, call: C) -> Result<R, Error>
    where
        C: Debug + PartialEq,
    {
        if self.failed {
            let actual = format!("{call:?}");
            self.calls.push(call);
            return Err(Error::ScriptUnexpected(actual));
        }
        let Some((expected, result)) = self.pending.pop_front() else {
            self.failed = true;
            let actual = format!("{call:?}");
            self.calls.push(call);
            return Err(Error::ScriptUnexpected(actual));
        };
        if expected != call {
            let message = (format!("{expected:?}"), format!("{call:?}"));
            self.failed = true;
            self.calls.push(call);
            return Err(Error::ScriptMismatch {
                expected: message.0,
                actual: message.1,
            });
        }
        self.calls.push(expected);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_calls_are_consumed_once() {
        let mut script = Scripted::<u8, &'static str>::new();
        script.expect_ok(1, "a");
        script.expect_ok(2, "b");
        assert_eq!(script.call(1).ok(), Some("a"));
        assert_eq!(script.call(2).ok(), Some("b"));
        assert!(script.is_complete());
        assert!(matches!(script.call(3), Err(Error::ScriptUnexpected(_))));
    }
    #[test]
    fn wrong_arguments_fail_the_script() {
        let mut script = Scripted::<u8, &'static str>::new();
        script.expect_ok(1, "a");
        assert!(matches!(script.call(9), Err(Error::ScriptMismatch { .. })));
        assert!(script.failed());
        assert_eq!(script.calls(), &[9]);
    }
    #[test]
    fn injected_error_is_terminal_and_later_calls_are_refused() {
        let mut script = Scripted::<u8, &'static str>::new();
        script.expect_err(1, Error::Timeout);
        script.expect_ok(2, "b");
        assert!(matches!(script.call(1), Err(Error::Timeout)));
        assert!(matches!(script.call(2), Err(Error::ScriptUnexpected(_))));
        assert_eq!(script.remaining(), 1);
        assert_eq!(script.calls(), &[1, 2]);
    }
    #[test]
    fn resume_allows_a_deliberate_retry() {
        let mut script = Scripted::<u8, &'static str>::new();
        script.expect_err(1, Error::Timeout);
        script.expect_ok(1, "retry");
        assert!(script.call(1).is_err());
        script.resume();
        assert_eq!(script.call(1).ok(), Some("retry"));
    }
}
