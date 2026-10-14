//! Minimal HTTP/1.1 GET client for localhost JSON endpoints (agent status API).
//! std-only: no TLS needed since we only ever talk to 127.0.0.1.

use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// GET `http://127.0.0.1:{port}{path}` and return the body.
pub fn get(port: u16, path: &str, timeout: Duration) -> Result<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port))
        .with_context(|| format!("connect 127.0.0.1:{port}"))?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes())?;
    let mut buf = String::new();
    s.read_to_string(&mut buf)?;
    let (head, body) = buf
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("bad http response"))?;
    if !head.starts_with("HTTP/1.1 200") {
        return Err(anyhow!(
            "http error: {}",
            head.lines().next().unwrap_or("?")
        ));
    }
    Ok(body.to_string())
}
