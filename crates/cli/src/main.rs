//! gg-lan-cli — operator companion for a running gg-lan-agent.

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "gg-lan-cli", version, about = "GG LAN operator CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show agent status: network, peers, paths, counters.
    Status {
        #[arg(long, default_value = "127.0.0.1:7800")]
        agent: String,
    },
    /// Ping a virtual IP through the agent's data plane.
    Ping {
        ip: String,
        #[arg(long, default_value = "127.0.0.1:7800")]
        agent: String,
    },
    /// List known peers (compact).
    Peers {
        #[arg(long, default_value = "127.0.0.1:7800")]
        agent: String,
    },
}

fn http_get(addr: &str, path: &str) -> Result<String> {
    let mut s = TcpStream::connect(addr).with_context(|| format!("connect {addr}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let host = addr.replace(':', "_"); // opaque token, host header unused
    write!(
        s,
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    )?;
    let mut buf = String::new();
    s.read_to_string(&mut buf)?;
    let body = buf
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .ok_or_else(|| anyhow!("bad http response"))?;
    Ok(body)
}

fn parse_status(addr: &str) -> Result<serde_json::Value> {
    let body = http_get(addr, "/status")?;
    serde_json::from_str(&body).context("parse status json")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Status { agent } => {
            let st = parse_status(&agent)?;
            println!("{}", serde_json::to_string_pretty(&st)?);
            Ok(())
        }
        Cmd::Ping { ip, agent } => {
            let body = http_get(&agent, &format!("/ping?ip={}", ip))?;
            let v: serde_json::Value = serde_json::from_str(&body).context("parse ping json")?;
            if v["ok"].as_bool() == Some(true) {
                println!("reply from {}: rtt = {} ms", ip, v["rtt_ms"]);
                Ok(())
            } else {
                Err(anyhow!("no reply: {}", v["error"].as_str().unwrap_or("?")))
            }
        }
        Cmd::Peers { agent } => {
            let st = parse_status(&agent)?;
            println!(
                "{:<18} {:<14} {:<9} {:>7} {:>12}",
                "NAME", "IP", "PATH", "RTT(ms)", "BYTES"
            );
            let mut rows: Vec<_> = st["peers"].as_array().cloned().unwrap_or_default();
            rows.sort_by_key(|p| p["ip"].as_str().unwrap_or("").to_string());
            for p in rows {
                println!(
                    "{:<18} {:<14} {:<9} {:>7} {:>12}",
                    p["name"].as_str().unwrap_or("?"),
                    p["ip"].as_str().unwrap_or("?"),
                    p["path"].as_str().unwrap_or("?"),
                    p["rtt_ms"]
                        .as_u64()
                        .map(|v| v.to_string())
                        .unwrap_or("-".into()),
                    (p["bytes_in"].as_u64().unwrap_or(0) + p["bytes_out"].as_u64().unwrap_or(0))
                        .to_string()
                );
            }
            Ok(())
        }
    }
}
