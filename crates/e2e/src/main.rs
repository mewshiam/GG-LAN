//! End-to-end test: launch server + N agents, wait for links, ping across
//! the mesh, then re-run in hub mode; verify analytics counters move.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bin(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    p.join(name)
}

fn spawn(args: &[&str], log: &str) -> Result<Proc> {
    let f = std::fs::File::create(log).with_context(|| format!("create {log}"))?;
    let child = Command::new(args[0])
        .args(&args[1..])
        .stdout(Stdio::from(f.try_clone()?))
        .stderr(Stdio::from(f))
        .spawn()
        .with_context(|| format!("spawn {}", args[0]))?;
    Ok(Proc(child))
}

fn http_get(addr: &str, path: &str, timeout: Duration) -> Result<String> {
    let mut s = TcpStream::connect(addr).context("connect")?;
    s.set_read_timeout(Some(timeout))?;
    write!(
        s,
        "GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        path
    )?;
    let _ = s.shutdown(Shutdown::Write);
    let mut buf = String::new();
    s.read_to_string(&mut buf)?;
    Ok(buf
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default())
}

fn get_json(addr: &str, path: &str) -> Result<serde_json::Value> {
    let body = http_get(addr, path, Duration::from_secs(10))?;
    serde_json::from_str(&body).context("json")
}

fn wait_for<F: Fn(&serde_json::Value) -> bool>(
    addr: &str,
    path: &str,
    cond: F,
    what: &str,
    deadline: Duration,
) -> Result<serde_json::Value> {
    let t0 = Instant::now();
    loop {
        if let Ok(v) = get_json(addr, path) {
            if cond(&v) {
                return Ok(v);
            }
        }
        if t0.elapsed() > deadline {
            bail!("timeout waiting for {what} at {addr}{path}");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

struct Node {
    name: String,
    #[allow(dead_code)]
    frame_port: u16,
    status: String,
    _proc: Proc,
}

fn run_phase(tag: &str, mode: &str, n_agents: usize, base_port: u16) -> Result<()> {
    let logdir = std::env::temp_dir().join("gg-lan-e2e");
    std::fs::create_dir_all(&logdir)?;

    let ctrl = format!("127.0.0.1:{}", base_port);
    let status = format!("127.0.0.1:{}", base_port + 100);

    let server = spawn(
        &[
            bin("gg-lan-server").to_str().unwrap(),
            "--ctrl",
            &ctrl,
            "--udp",
            &ctrl,
            "--status",
            &status,
            "--net-name",
            &format!("e2e-{tag}"),
            "--mode",
            mode,
            "--join-code",
            "e2ecode123",
            "--subnet",
            &format!("26.{}.0.0/16", 9 + (base_port % 20)),
        ],
        logdir.join(format!("{tag}-server.log")).to_str().unwrap(),
    )?;

    std::thread::sleep(Duration::from_millis(600));

    let mut nodes: Vec<Node> = Vec::new();
    for i in 0..n_agents {
        let name = format!("pc{}", i + 1);
        let frame_port = base_port + 200 + i as u16;
        let astat = format!("127.0.0.1:{}", base_port + 300 + i as u16);
        let proc = spawn(
            &[
                bin("gg-lan-agent").to_str().unwrap(),
                "--server",
                &ctrl,
                "--code",
                "e2ecode123",
                "--name",
                &name,
                "--frame-port",
                &frame_port.to_string(),
                "--status",
                &astat,
            ],
            logdir.join(format!("{tag}-{name}.log")).to_str().unwrap(),
        )?;
        nodes.push(Node {
            name,
            frame_port,
            status: astat,
            _proc: proc,
        });
    }
    let _ = server;

    // Wait until every agent sees all others with an established session.
    let expected = n_agents - 1;
    for n in nodes.iter() {
        wait_for(
            &n.status,
            "/status",
            |v| {
                v["peers"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .all(|p| p["path"] != "punching" && p["path"] != "unknown")
                    })
                    .unwrap_or(false)
                    && v["peers"].as_array().map(|a| a.len()) == Some(expected)
            },
            &format!("{} links", n.name),
            Duration::from_secs(40),
        )?;
    }

    // Determine each node's virtual IP, ping the others through node 1.
    let ips: Vec<String> = nodes
        .iter()
        .map(|n| {
            get_json(&n.status, "/status")
                .ok()
                .and_then(|v| v["my"]["ip"].as_str().map(|s| s.to_string()))
                .unwrap_or_default()
        })
        .collect();
    println!("[{tag}] virtual IPs: {:?}", ips);

    for n in nodes.iter().skip(1) {
        // find target ip by asking this node's status for peer list
        let st = get_json(&n.status, "/status")?;
        let peers = st["peers"].as_array().cloned().unwrap_or_default();
        let target = peers
            .iter()
            .find(|p| p["name"].as_str() == Some("pc1"))
            .and_then(|p| p["ip"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| ips[0].clone());
        let body = http_get(
            &n.status,
            &format!("/ping?ip={}", target),
            Duration::from_secs(6),
        )?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        if v["ok"].as_bool() != Some(true) {
            bail!("[{tag}] ping {} -> pc1 failed: {}", n.name, body);
        }
        println!("[{tag}] {} → pc1 ({}): {} ms", n.name, target, v["rtt_ms"]);
    }

    // Verify the direct/relay reporting and counters on pc1.
    let st = get_json(&nodes[0].status, "/status")?;
    for p in st["peers"].as_array().cloned().unwrap_or_default() {
        println!(
            "[{tag}] pc1 link: {} ({}) path={} rtt={}ms bytes_in={} bytes_out={}",
            p["name"].as_str().unwrap_or("?"),
            p["ip"].as_str().unwrap_or("?"),
            p["path"].as_str().unwrap_or("?"),
            p["rtt_ms"].as_u64().unwrap_or(0),
            p["bytes_in"].as_u64().unwrap_or(0),
            p["bytes_out"].as_u64().unwrap_or(0)
        );
        if p["bytes_in"].as_u64().unwrap_or(0) == 0 || p["bytes_out"].as_u64().unwrap_or(0) == 0 {
            bail!("[{tag}] traffic counters did not advance for {}", p["name"]);
        }
    }

    println!("[{tag}] PASS");
    Ok(())
}

fn main() -> Result<()> {
    let only = std::env::args().nth(1).unwrap_or_default();
    if only != "hub" {
        run_phase("mesh", "mesh", 3, 17100)?;
    }
    if only != "mesh" {
        run_phase("hub", "hub", 3, 27100)?;
    }
    println!("ALL E2E PHASES PASSED");
    Ok(())
}
