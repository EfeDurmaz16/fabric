//! A userspace relay for the M6 partition proof (ADR 0028): forwards UDP
//! datagrams (QUIC) and TCP bytes (the TLS fallback) between one loopback
//! port it owns and an upstream address. While a flag file exists it
//! drops every datagram in both directions and stops reading from the TCP
//! sockets, so bytes wait in the kernel's buffers and the sender is held
//! by flow control; resumed, the datagrams flow again and the held bytes
//! are forwarded in order. A TCP stream therefore sees silence, never a
//! gap, and a TLS record stream resumes where it stopped. It touches no
//! firewall and no system setting: a partition made with it is two
//! processes on one host that stop hearing each other, nothing more.
//!
//! Usage: `fabric-relay <upstream host:port> <pause flag path> <counters path>`
//!
//! It prints `listening 127.0.0.1:<port>` on its first line of standard
//! output, serves until it is killed, and rewrites the counters file every
//! 200 ms as `key=value` lines: `udp_forwarded_up`, `udp_forwarded_down`,
//! `udp_dropped_up`, `udp_dropped_down`, `tcp_forwarded_up`,
//! `tcp_forwarded_down`, `tcp_held_up`, `tcp_held_down`, where `up` is
//! toward the upstream and `down` toward the client, and `tcp_held` counts
//! the polls a stream spent waiting while paused. No dependency beyond std.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

/// How long a socket read waits before it looks at the flag again.
const POLL: Duration = Duration::from_millis(100);

/// The largest datagram the relay forwards: QUIC's are at most 1 500 bytes.
const DATAGRAM: usize = 65_535;

#[derive(Default)]
struct Counters {
    udp_forwarded: [AtomicU64; 2],
    udp_dropped: [AtomicU64; 2],
    tcp_forwarded: [AtomicU64; 2],
    tcp_held: [AtomicU64; 2],
}

/// Toward the upstream, or toward the client.
#[derive(Clone, Copy)]
enum Way {
    Up = 0,
    Down = 1,
}

struct Relay {
    upstream: SocketAddr,
    flag: PathBuf,
    counters: Counters,
}

impl Relay {
    fn paused(&self) -> bool {
        self.flag.exists()
    }

    fn count_udp(&self, way: Way, forwarded: bool) {
        let slot = if forwarded {
            &self.counters.udp_forwarded
        } else {
            &self.counters.udp_dropped
        };
        slot[way as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Counts bytes forwarded, or polls a stream spent held while paused.
    fn count_tcp(&self, way: Way, forwarded: bool, count: u64) {
        let slot = if forwarded {
            &self.counters.tcp_forwarded
        } else {
            &self.counters.tcp_held
        };
        slot[way as usize].fetch_add(count, Ordering::Relaxed);
    }

    fn write_counters(&self, path: &Path) {
        let c = &self.counters;
        let text = format!(
            "udp_forwarded_up={}\nudp_forwarded_down={}\nudp_dropped_up={}\nudp_dropped_down={}\n\
             tcp_forwarded_up={}\ntcp_forwarded_down={}\ntcp_held_up={}\ntcp_held_down={}\n",
            c.udp_forwarded[0].load(Ordering::Relaxed),
            c.udp_forwarded[1].load(Ordering::Relaxed),
            c.udp_dropped[0].load(Ordering::Relaxed),
            c.udp_dropped[1].load(Ordering::Relaxed),
            c.tcp_forwarded[0].load(Ordering::Relaxed),
            c.tcp_forwarded[1].load(Ordering::Relaxed),
            c.tcp_held[0].load(Ordering::Relaxed),
            c.tcp_held[1].load(Ordering::Relaxed),
        );
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Binds UDP and TCP on one free loopback port.
fn bind_both() -> std::io::Result<(UdpSocket, TcpListener)> {
    let mut last = None;
    for _ in 0..64 {
        let udp = UdpSocket::bind("127.0.0.1:0")?;
        let port = udp.local_addr()?.port();
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(tcp) => return Ok((udp, tcp)),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no port free for both UDP and TCP")))
}

/// Forwards datagrams from `front` to one upstream socket per client, and
/// their answers back; every datagram is dropped while paused.
fn serve_udp(relay: &Arc<Relay>, front: UdpSocket) {
    let front = Arc::new(front);
    let _ = front.set_read_timeout(Some(POLL));
    let mut clients: HashMap<SocketAddr, Arc<UdpSocket>> = HashMap::new();
    let mut buf = vec![0u8; DATAGRAM];
    loop {
        let (len, client) = match front.recv_from(&mut buf) {
            Ok(got) => got,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => return,
        };
        if relay.paused() {
            relay.count_udp(Way::Up, false);
            continue;
        }
        let Some(up) = upstream_for(relay, &front, &mut clients, client) else {
            continue;
        };
        if up.send(&buf[..len]).is_ok() {
            relay.count_udp(Way::Up, true);
        }
    }
}

/// The upstream socket that stands for `client`, made on its first
/// datagram with a thread that carries the answers back.
fn upstream_for(
    relay: &Arc<Relay>,
    front: &Arc<UdpSocket>,
    clients: &mut HashMap<SocketAddr, Arc<UdpSocket>>,
    client: SocketAddr,
) -> Option<Arc<UdpSocket>> {
    if let Some(up) = clients.get(&client) {
        return Some(Arc::clone(up));
    }
    let up = UdpSocket::bind("127.0.0.1:0").ok()?;
    up.connect(relay.upstream).ok()?;
    let _ = up.set_read_timeout(Some(POLL));
    let up = Arc::new(up);
    clients.insert(client, Arc::clone(&up));
    let (relay, front, up_reader) = (Arc::clone(relay), Arc::clone(front), Arc::clone(&up));
    thread::spawn(move || {
        let mut buf = vec![0u8; DATAGRAM];
        loop {
            match up_reader.recv(&mut buf) {
                Ok(len) => {
                    if relay.paused() {
                        relay.count_udp(Way::Down, false);
                        continue;
                    }
                    if front.send_to(&buf[..len], client).is_ok() {
                        relay.count_udp(Way::Down, true);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return,
            }
        }
    });
    Some(up)
}

/// Copies bytes one way between two TCP streams. While paused it does not
/// read: the bytes wait in the socket's buffer and TCP's flow control
/// holds the sender, so nothing is lost and the stream resumes in order;
/// the connection stays open and the peers see silence.
fn pipe(relay: &Relay, mut from: TcpStream, mut to: TcpStream, way: Way) {
    let _ = from.set_read_timeout(Some(POLL));
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        if relay.paused() {
            relay.count_tcp(way, false, 1);
            thread::sleep(POLL);
            continue;
        }
        match from.read(&mut buf) {
            Ok(0) => {
                let _ = to.shutdown(std::net::Shutdown::Write);
                return;
            }
            Ok(n) => {
                let bytes = u64::try_from(n).unwrap_or(u64::MAX);
                if to.write_all(&buf[..n]).is_err() {
                    return;
                }
                relay.count_tcp(way, true, bytes);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
}

fn serve_tcp(relay: &Arc<Relay>, listener: &TcpListener) {
    for client in listener.incoming().flatten() {
        let Ok(up) = TcpStream::connect(relay.upstream) else {
            continue;
        };
        let (Ok(client_back), Ok(up_back)) = (client.try_clone(), up.try_clone()) else {
            continue;
        };
        let r = Arc::clone(relay);
        thread::spawn(move || pipe(&r, client, up, Way::Up));
        let r = Arc::clone(relay);
        thread::spawn(move || pipe(&r, up_back, client_back, Way::Down));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [upstream, flag, counters] = args.as_slice() else {
        let _ = writeln!(
            std::io::stderr(),
            "usage: fabric-relay <upstream host:port> <pause flag path> <counters path>"
        );
        return ExitCode::from(2);
    };
    let Ok(upstream) = upstream.parse::<SocketAddr>() else {
        let _ = writeln!(std::io::stderr(), "{upstream} is not a host:port");
        return ExitCode::from(2);
    };
    let (udp, tcp) = match bind_both() {
        Ok(bound) => bound,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "bind: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(addr) = udp.local_addr() else {
        return ExitCode::FAILURE;
    };
    let relay = Arc::new(Relay {
        upstream,
        flag: PathBuf::from(flag),
        counters: Counters::default(),
    });
    let counters_path = PathBuf::from(counters);
    relay.write_counters(&counters_path);
    {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "listening {addr}");
        let _ = out.flush();
    }
    let writer = Arc::clone(&relay);
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(200));
            writer.write_counters(&counters_path);
        }
    });
    let r = Arc::clone(&relay);
    thread::spawn(move || serve_tcp(&r, &tcp));
    serve_udp(&relay, udp);
    ExitCode::SUCCESS
}
