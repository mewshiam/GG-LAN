//! Virtual NIC abstraction: TAP (Linux) for production, userspace frame-bridge
//! for sandboxes / tests / OSes without a TAP driver.

use anyhow::{anyhow, Result};
use std::sync::Arc;

/// Callback for frames produced by the local host (outbound).
/// ⚠ Invoked from arbitrary threads — hop into your async runtime yourself.
pub type FrameTx = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// A virtual network interface the agent bridges to the mesh.
pub trait VirtualNic: Send + Sync {
    /// Push a frame down to the local OS/application (inbound from the mesh).
    fn deliver(&self, frame: &[u8]);
    /// Human-readable identity.
    fn describe(&self) -> String;
}

/// Userspace NIC: a TCP server speaking `[u32 LE len][ethernet frame]`.
/// Test applications (or the built-in demo host) connect and exchange raw
/// frames. Implemented with plain std sockets + threads so the handler may be
/// invoked from any thread (the agent hops into its tokio runtime itself).
pub struct UserspaceNic {
    port: u16,
    clients: std::sync::Mutex<Vec<(u64, std::net::TcpStream)>>,
    handler: FrameTx,
    next_id: std::sync::atomic::AtomicU64,
}

impl UserspaceNic {
    /// Bind on 127.0.0.1:port (port 0 → kernel-assigned). Handler runs on a
    /// dedicated reader thread per connected client.
    pub fn bind(port: u16, handler: FrameTx) -> Result<Arc<UserspaceNic>> {
        use std::net::TcpListener;
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let nic = Arc::new(UserspaceNic {
            port,
            clients: std::sync::Mutex::new(Vec::new()),
            handler,
            next_id: std::sync::atomic::AtomicU64::new(1),
        });
        let n = Arc::clone(&nic);
        std::thread::Builder::new()
            .name("userspace-nic-accept".into())
            .spawn(move || {
                for sock in listener.incoming() {
                    match sock {
                        Ok(s) => n.run_client(s),
                        Err(_) => continue,
                    }
                }
            })
            .map_err(|e| anyhow!("thread spawn: {e}"))?;
        Ok(nic)
    }

    fn run_client(self: &Arc<Self>, sock: std::net::TcpStream) {
        let n = Arc::clone(self);
        std::thread::Builder::new()
            .name("userspace-nic-read".into())
            .spawn(move || {
                use std::io::Read;
                let id = n.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                {
                    let _ = sock.set_nodelay(true);
                    let mut cs = n.clients.lock().unwrap();
                    cs.push((id, sock.try_clone().unwrap()));
                }
                let mut rd = std::io::BufReader::new(sock);
                let mut hdr = [0u8; 4];
                loop {
                    if rd.read_exact(&mut hdr).is_err() {
                        break;
                    }
                    let len = u32::from_le_bytes(hdr) as usize;
                    if len == 0 || len > 16 * 1024 {
                        break;
                    }
                    let mut frame = vec![0u8; len];
                    if rd.read_exact(&mut frame).is_err() {
                        break;
                    }
                    (n.handler)(&frame);
                }
                let mut cs = n.clients.lock().unwrap();
                cs.retain(|(cid, _)| *cid != id);
            })
            .map_err(|e| anyhow!("thread spawn: {e}"))
            .ok();
    }
}

impl VirtualNic for UserspaceNic {
    fn deliver(&self, frame: &[u8]) {
        let mut msg = Vec::with_capacity(4 + frame.len());
        msg.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        msg.extend_from_slice(frame);
        let clients: Vec<(u64, std::net::TcpStream)> = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .map(|(i, s)| (*i, s.try_clone().unwrap()))
            .collect();
        for (_, mut c) in clients {
            use std::io::Write;
            let _ = c.write_all(&msg);
            let _ = c.flush();
        }
    }

    fn describe(&self) -> String {
        format!("userspace-nic tcp/{} (frame bridge)", self.port)
    }
}

impl UserspaceNic {
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Linux TAP NIC (requires /dev/net/tun + CAP_NET_ADMIN). Enabled at build via
/// the `tap` feature and selected with `--mode tap` on the agent.
#[cfg(all(target_os = "linux", feature = "tap"))]
pub mod tap {
    use super::*;
    use std::os::fd::AsRawFd;

    pub struct TapNic {
        fd: std::sync::Mutex<i32>,
        name: String,
        handler: FrameTx,
        mtu: usize,
    }

    impl TapNic {
        /// Open a persistent TAP device with the given name (or kernel-assigned).
        pub fn open(name: &str, handler: FrameTx) -> Result<TapNic> {
            // SAFETY: standard TUNSETIFF ioctls.
            let fd = unsafe {
                let f = libc::open(
                    b"/dev/net/tun\0".as_ptr() as *const i8,
                    libc::O_RDWR | libc::O_NONBLOCK,
                );
                if f < 0 {
                    return Err(anyhow!("cannot open /dev/net/tun (root?)"));
                }
                let mut iff: libc::ifreq = std::mem::zeroed();
                for (i, b) in name.bytes().enumerate().take(15) {
                    iff.ifr_name[i] = b as i8;
                }
                let flags = libc::IFF_TAP | libc::IFF_NO_PI;
                iff.ifr_ifru.ifru_flags = flags as i16;
                let r = libc::ioctl(f, libc::TUNSETIFF, &iff);
                if r < 0 {
                    libc::close(f);
                    return Err(anyhow!("TUNSETIFF failed (permission?)"));
                }
                f
            };
            let dev = {
                let ifr = std::mem::zeroed::<libc::ifreq>();
                let _ = ifr;
                unsafe {
                    let mut nm = [0u8; 16];
                    let ptr = &nm as *const _;
                    let _ = ptr;
                    let mut iff: libc::ifreq = std::mem::zeroed();
                    if libc::ioctl(fd, libc::TUNGETIFF, &mut iff) == 0 {
                        let bytes = &iff.ifr_name[..];
                        let end = bytes.iter().position(|&c| c == 0).unwrap_or(15);
                        String::from_utf8_lossy(&bytes[..end]).to_string()
                    } else {
                        name.to_string()
                    }
                }
            };
            let nic = TapNic {
                fd: std::sync::Mutex::new(fd),
                name: dev,
                handler,
                mtu: 10_240,
            };
            // reader thread
            let fd2 = fd;
            let h = Arc::clone(&handler);
            std::thread::Builder::new()
                .name("tap-read".into())
                .spawn(move || {
                    let mut buf = vec![0u8; 65536];
                    loop {
                        let n = unsafe {
                            libc::read(fd2, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
                        };
                        if n <= 0 {
                            if n < 0
                                && std::io::Error::last_os_error().raw_os_error()
                                    == Some(libc::EAGAIN)
                            {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                                continue;
                            }
                            break;
                        }
                        let frame = &buf[..n as usize];
                        if frame.len() > 4 {
                            h(&frame[4.min(frame.len())..]);
                        }
                    }
                })
                .map_err(|e| anyhow!("thread: {e}"))?;
            Ok(nic)
        }

        fn write_frame(&self, frame: &[u8]) -> Result<()> {
            let fd = *self.fd.lock().unwrap();
            let n = unsafe { libc::write(fd, frame.as_ptr() as *const libc::c_void, frame.len()) };
            if n < 0 {
                return Err(anyhow!("tap write failed"));
            }
            Ok(())
        }
    }

    impl VirtualNic for TapNic {
        fn deliver(&self, frame: &[u8]) {
            let _ = self.write_frame(frame);
        }

        fn describe(&self) -> String {
            format!("tap-nic {} (mtu {})", self.name, self.mtu)
        }
    }
}
