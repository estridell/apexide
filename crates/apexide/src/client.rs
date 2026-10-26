//! Tiny blocking HTTP/1.1 client for the daemon's Unix socket.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use serde_json::Value;

/// The daemon socket could not be reached.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DaemonDown(String);

pub struct Client {
    pub socket: PathBuf,
}

pub enum Body {
    None,
    Json(Value),
    Bytes(Vec<u8>),
}

impl Client {
    pub fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.request("GET", path, Body::None)
    }

    pub fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.request("POST", path, Body::Json(body))
    }

    pub fn put(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.request("PUT", path, Body::Json(body))
    }

    pub fn request(&self, method: &str, path: &str, body: Body) -> anyhow::Result<Value> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| {
            DaemonDown(format!(
                "cannot reach the apexide daemon at {} ({e}). \
                 Start it with `apexide daemon`, or `systemctl --user enable --now apexide`.",
                self.socket.display()
            ))
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;

        let (ctype, payload) = match body {
            Body::None => (None, Vec::new()),
            Body::Json(v) => (Some("application/json"), serde_json::to_vec(&v)?),
            Body::Bytes(b) => (Some("application/octet-stream"), b),
        };
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: apexide\r\nConnection: close\r\n");
        if let Some(ct) = ctype {
            req += &format!(
                "Content-Type: {ct}\r\nContent-Length: {}\r\n",
                payload.len()
            );
        }
        req += "\r\n";
        stream.write_all(req.as_bytes())?;
        stream.write_all(&payload)?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .context("reading daemon response")?;
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| anyhow!("malformed response"))?;
        let head = String::from_utf8_lossy(&raw[..split]);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow!("malformed status line"))?;
        let body = &raw[split + 4..];
        let value: Value = if body.is_empty() {
            Value::Null
        } else {
            // Errors produced outside our handlers may be plain text.
            serde_json::from_slice(body).unwrap_or_else(
                |_| serde_json::json!({ "error": String::from_utf8_lossy(body).trim() }),
            )
        };
        if !(200..300).contains(&status) {
            let msg = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request failed");
            bail!("{msg} (HTTP {status})");
        }
        Ok(value)
    }
}
