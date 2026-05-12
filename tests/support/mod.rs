#![allow(dead_code)]

use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use tempfile::{tempdir, TempDir};

pub fn temp_workspace() -> TempDir {
    tempdir().expect("tempdir")
}

pub fn v8_runner_command() -> Command {
    Command::cargo_bin("v8-runner").expect("binary")
}

pub fn v8_runner_binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("v8-runner")
}

pub fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

pub fn write_shell_script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    make_executable(path);
}

pub fn write_shell_script_atomically(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    let staged = path.with_extension("tmp");
    fs::write(&staged, format!("#!/bin/sh\n{body}\n")).expect("write script");
    make_executable(&staged);
    fs::rename(&staged, path).expect("rename script");
}

pub fn wait_until<F>(timeout: Duration, interval: Duration, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(interval);
    }
    condition()
}

pub fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    wait_until(timeout, Duration::from_millis(50), || path.exists())
}

pub fn wait_for_received_line<F>(
    rx: &mpsc::Receiver<String>,
    timeout: Duration,
    interval: Duration,
    mut predicate: F,
) -> bool
where
    F: FnMut(&str) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.recv_timeout(interval) {
            Ok(line) if predicate(&line) => return true,
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
    false
}

pub fn read_line_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.lines().count())
        .unwrap_or(0)
}

pub fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind free port")
        .local_addr()
        .expect("local addr")
        .port()
}

pub async fn wait_until_async<F>(attempts: usize, interval: Duration, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    for _ in 0..attempts {
        if condition() {
            return true;
        }
        tokio::time::sleep(interval).await;
    }
    condition()
}

pub async fn wait_until_async_condition<F, Fut>(
    attempts: usize,
    interval: Duration,
    mut condition: F,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..attempts {
        if condition().await {
            return true;
        }
        tokio::time::sleep(interval).await;
    }
    condition().await
}

pub async fn wait_for_log_contains(path: &Path, needle: &str) {
    if wait_until_async(100, Duration::from_millis(20), || {
        fs::read_to_string(path)
            .map(|contents| contents.contains(needle))
            .unwrap_or(false)
    })
    .await
    {
        return;
    }

    panic!("timed out waiting for '{needle}' in {}", path.display());
}

pub async fn wait_for_line_count(path: &Path, expected: usize) {
    if wait_until_async(300, Duration::from_millis(10), || {
        read_line_count(path) >= expected
    })
    .await
    {
        return;
    }

    panic!(
        "timed out waiting for {expected} lines in {}",
        path.display()
    );
}
