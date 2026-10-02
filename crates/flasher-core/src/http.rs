//! Narrow HTTPS seam. Production uses pinned ureq; tests script responses.
//!
//! The client performs one request per call without following redirects; the
//! bounded redirect policy remains in [`crate::assets::Cache::download`].
use crate::{Cancellation, Error, script::Scripted};
use std::{
    fmt::{Debug, Formatter},
    io::Read,
    sync::{Mutex, MutexGuard, PoisonError},
};
use url::Url;

/// One HTTP response with an owned body stream.
pub struct HttpResponse {
    pub status: u16,
    pub location: Option<String>,
    pub content_length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}
impl Debug for HttpResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("location", &self.location)
            .field("content_length", &self.content_length)
            .field("body", &"<stream>")
            .finish()
    }
}

/// Retrieves one URL. Implementations must enforce HTTPS and public hosts.
pub trait HttpClient: Send + Sync + Debug {
    /// # Errors
    /// Returns cancellation, timeout, protocol, status or transport errors.
    fn get(&self, url: &Url, cancel: &Cancellation) -> Result<HttpResponse, Error>;
}

/// Production client: pinned ureq agent with public-network and cancellation checks.
#[derive(Clone, Copy, Debug, Default)]
pub struct UreqHttpClient;
impl HttpClient for UreqHttpClient {
    fn get(&self, url: &Url, cancel: &Cancellation) -> Result<HttpResponse, Error> {
        cancel.check()?;
        let agent = crate::transport::agent(cancel);
        let response = agent
            .get(url.as_str())
            .header("Accept-Encoding", "identity")
            .call()
            .map_err(|error| match cancel.check() {
                Err(cancelled) => cancelled,
                Ok(()) => {
                    if matches!(error, ureq::Error::Timeout(_)) {
                        Error::Timeout
                    } else {
                        Error::Network
                    }
                }
            })?;
        if response.headers().contains_key("content-encoding") {
            return Err(Error::Network);
        }
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_length = response
            .headers()
            .get("content-length")
            .map(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|text| text.parse::<u64>().ok())
                    .ok_or(Error::Length)
            })
            .transpose()?;
        let body: Box<dyn Read + Send> = Box::new(response.into_body().into_reader());
        Ok(HttpResponse {
            status,
            location,
            content_length,
            body,
        })
    }
}

/// A scripted response body and metadata for deterministic tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptedResponse {
    pub status: u16,
    pub location: Option<String>,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}
impl ScriptedResponse {
    /// A complete `200` response declaring the exact body length.
    #[must_use]
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        let content_length = Some(u64::try_from(body.len()).unwrap_or(u64::MAX));
        Self {
            status: 200,
            location: None,
            content_length,
            body,
        }
    }
    #[must_use]
    pub const fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
    #[must_use]
    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
    #[must_use]
    pub const fn content_length(mut self, length: u64) -> Self {
        self.content_length = Some(length);
        self
    }
    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}
impl From<ScriptedResponse> for HttpResponse {
    fn from(response: ScriptedResponse) -> Self {
        Self {
            status: response.status,
            location: response.location,
            content_length: response.content_length,
            body: Box::new(std::io::Cursor::new(response.body)),
        }
    }
}

/// Deterministic client for tests: ordered expectations and terminal errors.
#[derive(Debug)]
pub struct ScriptedHttpClient {
    script: Mutex<Scripted<Url, HttpResponse>>,
}
impl ScriptedHttpClient {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            script: Mutex::new(Scripted::new()),
        }
    }
    /// Registers the next expected URL and a complete scripted response.
    pub fn expect_response(&self, url: &Url, response: ScriptedResponse) {
        self.lock().expect_ok(url.clone(), response.into());
    }
    /// Registers the next expected URL and an injected transport failure.
    pub fn expect_error(&self, url: &Url, error: Error) {
        self.lock().expect_err(url.clone(), error);
    }
    #[must_use]
    pub fn calls(&self) -> Vec<Url> {
        self.lock().calls().to_vec()
    }
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.lock().remaining()
    }
    fn lock(&self) -> MutexGuard<'_, Scripted<Url, HttpResponse>> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
impl Default for ScriptedHttpClient {
    fn default() -> Self {
        Self::new()
    }
}
impl HttpClient for ScriptedHttpClient {
    fn get(&self, url: &Url, cancel: &Cancellation) -> Result<HttpResponse, Error> {
        if let Err(cancelled) = cancel.check() {
            self.lock().poison();
            return Err(cancelled);
        }
        self.lock().call(url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripted_client_rejects_unexpected_and_mismatched_requests() -> Result<(), Error> {
        let url = Url::parse("https://example.invalid/a").map_err(|_| Error::State)?;
        let other = Url::parse("https://example.invalid/b").map_err(|_| Error::State)?;
        let client = ScriptedHttpClient::new();
        client.expect_response(&url, ScriptedResponse::ok(b"ok".to_vec()));
        assert!(matches!(
            client.get(&other, &Cancellation::default()),
            Err(Error::ScriptMismatch { .. })
        ));
        assert!(matches!(
            client.get(&url, &Cancellation::default()),
            Err(Error::ScriptUnexpected(_))
        ));
        Ok(())
    }
    #[test]
    fn scripted_client_checks_cancellation_before_serving() -> Result<(), Error> {
        let url = Url::parse("https://example.invalid/a").map_err(|_| Error::State)?;
        let client = ScriptedHttpClient::new();
        client.expect_response(&url, ScriptedResponse::ok(Vec::new()));
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(client.get(&url, &cancel), Err(Error::Cancelled)));
        assert_eq!(client.remaining(), 1);
        Ok(())
    }
}
