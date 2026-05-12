use std::io::Read;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::StatusCode;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const READ_BUFFER_SIZE: usize = 64 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP client setup failed: {0}")]
    Client(reqwest::Error),

    #[error("HTTP GET {url} failed: {source}")]
    Request { url: String, source: reqwest::Error },

    #[error("HTTP GET {url} returned status {status}")]
    Status { url: String, status: StatusCode },

    #[error("HTTP response read failed for {url}: {source}")]
    Read { url: String, source: std::io::Error },

    #[error(
        "HTTP response for {url} exceeds maximum download size {max_bytes} bytes: {size_bytes} bytes"
    )]
    ResponseTooLarge {
        url: String,
        size_bytes: u64,
        max_bytes: u64,
    },

    #[error("HTTP download timed out after {timeout_ms}ms")]
    TimedOut { timeout_ms: u64 },

    #[error("HTTP download was cancelled")]
    Cancelled,

    #[error("response is not UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

pub fn get_text(
    url: &str,
    timeout: Option<Duration>,
    cancellation: &CancellationToken,
) -> Result<String, DownloadError> {
    let bytes = get_bytes(url, timeout, cancellation)?;
    String::from_utf8(bytes).map_err(DownloadError::InvalidUtf8)
}

pub fn get_bytes(
    url: &str,
    timeout: Option<Duration>,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, DownloadError> {
    if timeout.is_some_and(|value| value.is_zero()) {
        return Err(DownloadError::TimedOut { timeout_ms: 0 });
    }

    let started = Instant::now();
    let mut last_error = None;

    for attempt in 1..=RETRY_ATTEMPTS {
        ensure_not_cancelled(cancellation)?;
        let request_timeout = remaining_budget(timeout, started)?;
        let client = build_client(request_timeout)?;

        match download_once(&client, url, timeout, started, cancellation) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if attempt < RETRY_ATTEMPTS && error.is_retryable() => {
                last_error = Some(error);
                sleep_before_retry(cancellation, timeout, started)?;
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_error.unwrap_or(DownloadError::Cancelled))
}

fn build_client(timeout: Option<Duration>) -> Result<Client, DownloadError> {
    let connect_timeout = timeout
        .map(|value| value.min(CONNECT_TIMEOUT))
        .unwrap_or(CONNECT_TIMEOUT);
    let mut builder = Client::builder()
        .connect_timeout(connect_timeout)
        .user_agent("v8-runner");
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(DownloadError::Client)
}

fn download_once(
    client: &Client,
    url: &str,
    timeout: Option<Duration>,
    started: Instant,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, DownloadError> {
    ensure_not_cancelled(cancellation)?;
    let url_text = url.to_owned();
    let mut response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|source| DownloadError::Request {
            url: url_text.clone(),
            source,
        })?;

    if !response.status().is_success() {
        return Err(DownloadError::Status {
            url: url_text,
            status: response.status(),
        });
    }

    let content_length = response.content_length();
    if let Some(size_bytes) = content_length {
        ensure_allowed_download_size(&url_text, size_bytes)?;
    }
    let capacity = content_length
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default();
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = vec![0_u8; READ_BUFFER_SIZE];

    loop {
        ensure_not_cancelled(cancellation)?;
        let _ = remaining_budget(timeout, started)?;
        let read = response
            .read(&mut buffer)
            .map_err(|source| DownloadError::Read {
                url: url_text.clone(),
                source,
            })?;
        if read == 0 {
            return Ok(bytes);
        }
        let next_len = bytes
            .len()
            .checked_add(read)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(u64::MAX);
        ensure_allowed_download_size(&url_text, next_len)?;
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn ensure_allowed_download_size(url: &str, size_bytes: u64) -> Result<(), DownloadError> {
    if size_bytes > MAX_DOWNLOAD_BYTES {
        Err(DownloadError::ResponseTooLarge {
            url: url.to_owned(),
            size_bytes,
            max_bytes: MAX_DOWNLOAD_BYTES,
        })
    } else {
        Ok(())
    }
}

fn remaining_budget(
    timeout: Option<Duration>,
    started: Instant,
) -> Result<Option<Duration>, DownloadError> {
    let Some(limit) = timeout else {
        return Ok(None);
    };
    limit
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .map(Some)
        .ok_or_else(|| DownloadError::TimedOut {
            timeout_ms: limit.as_millis() as u64,
        })
}

fn sleep_before_retry(
    cancellation: &CancellationToken,
    timeout: Option<Duration>,
    started: Instant,
) -> Result<(), DownloadError> {
    let delay = remaining_budget(timeout, started)?
        .map(|remaining| remaining.min(RETRY_DELAY))
        .unwrap_or(RETRY_DELAY);
    let until = Instant::now() + delay;
    while Instant::now() < until {
        ensure_not_cancelled(cancellation)?;
        std::thread::sleep(
            Duration::from_millis(25).min(until.saturating_duration_since(Instant::now())),
        );
    }
    Ok(())
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), DownloadError> {
    if cancellation.is_cancelled() {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

impl DownloadError {
    fn is_retryable(&self) -> bool {
        match self {
            DownloadError::Request { source, .. } => source.is_timeout() || source.is_connect(),
            DownloadError::Read { .. } => true,
            DownloadError::Status { status, .. } => status.is_server_error(),
            DownloadError::Client(_)
            | DownloadError::ResponseTooLarge { .. }
            | DownloadError::TimedOut { .. }
            | DownloadError::Cancelled
            | DownloadError::InvalidUtf8(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_size_limit_accepts_boundary_size() {
        ensure_allowed_download_size("https://example.invalid/file", MAX_DOWNLOAD_BYTES)
            .expect("boundary size is accepted");
    }

    #[test]
    fn download_size_limit_rejects_oversized_response() {
        let error =
            ensure_allowed_download_size("https://example.invalid/file", MAX_DOWNLOAD_BYTES + 1)
                .expect_err("oversized response is rejected");

        assert!(matches!(
            error,
            DownloadError::ResponseTooLarge {
                size_bytes,
                max_bytes,
                ..
            } if size_bytes == MAX_DOWNLOAD_BYTES + 1 && max_bytes == MAX_DOWNLOAD_BYTES
        ));
    }
}
