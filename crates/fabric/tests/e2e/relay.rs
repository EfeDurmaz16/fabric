//! The userspace relay of the M6 partition proof (ADR 0028), as the test
//! owns it: `fabric-relay` started on a port of its own in front of an
//! upstream address, paused by a flag file and read through its counters
//! file. This is a two-process partition on one host, not two hosts: no
//! firewall and no system setting is touched.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::world::{Env, Fallible};

/// Datagrams forwarded and dropped, TCP bytes forwarded and TCP polls held
/// while paused, per direction, `[up, down]` where up is toward the upstream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayCounters {
    pub udp_forwarded: [u64; 2],
    pub udp_dropped: [u64; 2],
    pub tcp_forwarded: [u64; 2],
    pub tcp_held: [u64; 2],
}

impl RelayCounters {
    fn parse(text: &str) -> RelayCounters {
        let mut c = RelayCounters::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value: u64 = value.trim().parse().unwrap_or(0);
            match key {
                "udp_forwarded_up" => c.udp_forwarded[0] = value,
                "udp_forwarded_down" => c.udp_forwarded[1] = value,
                "udp_dropped_up" => c.udp_dropped[0] = value,
                "udp_dropped_down" => c.udp_dropped[1] = value,
                "tcp_forwarded_up" => c.tcp_forwarded[0] = value,
                "tcp_forwarded_down" => c.tcp_forwarded[1] = value,
                "tcp_held_up" => c.tcp_held[0] = value,
                "tcp_held_down" => c.tcp_held[1] = value,
                _ => {}
            }
        }
        c
    }

    /// Forwarded in both directions, UDP and TCP together.
    pub fn forwarded(&self) -> u64 {
        self.udp_forwarded.iter().chain(&self.tcp_forwarded).sum()
    }

    /// Datagrams dropped in both directions while paused.
    pub fn dropped(&self) -> u64 {
        self.udp_dropped.iter().sum()
    }
}

/// A running relay.
pub struct Relay {
    /// The loopback address peers dial instead of the upstream.
    pub addr: SocketAddr,
    child: Child,
    flag: PathBuf,
    counters: PathBuf,
}

impl Relay {
    /// Starts `fabric-relay` in front of `upstream`, with its flag and
    /// counters files under `env`'s root, and waits for its address.
    pub fn start(env: &Env, name: &str, upstream: SocketAddr) -> Fallible<Relay> {
        let dir = env.root.join(format!("relay-{name}"));
        std::fs::create_dir_all(&dir)?;
        let flag = dir.join("paused");
        let counters = dir.join("counters");
        let bin = env.fabric().with_file_name("fabric-relay");
        let mut child = Command::new(&bin)
            .arg(upstream.to_string())
            .arg(&flag)
            .arg(&counters)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("{}: {e}", bin.display()))?;
        let stdout = child.stdout.take().ok_or("the relay has no stdout")?;
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line)?;
        let addr = line
            .strip_prefix("listening ")
            .ok_or_else(|| format!("the relay said {line:?}"))?
            .trim()
            .parse::<SocketAddr>()?;
        Ok(Relay {
            addr,
            child,
            flag,
            counters,
        })
    }

    /// Drops every datagram and holds every TCP byte in both directions
    /// from now on; the sockets stay open.
    pub fn pause(&self) -> Fallible {
        std::fs::write(&self.flag, b"paused\n")?;
        // The relay looks at the flag per datagram; give its readers one poll.
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    /// Forwards again.
    pub fn resume(&self) -> Fallible {
        match std::fs::remove_file(&self.flag) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    /// The counters as the relay last wrote them (every 200 ms).
    pub fn counters(&self) -> Fallible<RelayCounters> {
        std::thread::sleep(Duration::from_millis(250));
        Ok(RelayCounters::parse(&std::fs::read_to_string(
            &self.counters,
        )?))
    }

    /// The counters once two consecutive readings agree: a baseline that
    /// no datagram still in flight when it was taken can move, or a
    /// failure after `timeout`.
    pub fn settled(&self, timeout: Duration) -> Fallible<RelayCounters> {
        let start = Instant::now();
        let mut last = self.counters()?;
        loop {
            let next = self.counters()?;
            if next == last {
                return Ok(next);
            }
            if start.elapsed() > timeout {
                return Err(
                    format!("the relay's counters never settled: {last:?} then {next:?}").into(),
                );
            }
            last = next;
        }
    }

    /// Waits until `what` holds of the counters, or fails after `timeout`.
    pub fn wait_until(
        &self,
        timeout: Duration,
        what: impl Fn(&RelayCounters) -> bool,
        name: &str,
    ) -> Fallible<RelayCounters> {
        let start = Instant::now();
        loop {
            let c = self.counters()?;
            if what(&c) {
                return Ok(c);
            }
            if start.elapsed() > timeout {
                return Err(format!("the relay never reached {name}: {c:?}").into());
            }
        }
    }

    /// Stops the relay.
    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The relay forwards UDP and TCP, drops both while paused and keeps
/// count, against an echo server the test owns.
#[test]
fn the_relay_forwards_pauses_and_counts() -> Fallible {
    use std::io::{Read, Write};
    use std::net::{TcpListener, UdpSocket};
    let env = Env::new()?;
    // A UDP echo and a TCP echo on one port.
    let udp = UdpSocket::bind("127.0.0.1:0")?;
    let port = udp.local_addr()?.port();
    let tcp = TcpListener::bind(("127.0.0.1", port))?;
    let upstream = udp.local_addr()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = udp.recv_from(&mut buf) {
            let _ = udp.send_to(&buf[..n], from);
        }
    });
    std::thread::spawn(move || {
        for stream in tcp.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 || stream.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let relay = Relay::start(&env, "echo", upstream)?;

    // Forwarding: a datagram and a TCP line come back through the relay.
    let client = UdpSocket::bind("127.0.0.1:0")?;
    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    client.send_to(b"ping", relay.addr)?;
    let mut buf = [0u8; 16];
    let (n, _) = client.recv_from(&mut buf)?;
    assert_eq!(&buf[..n], b"ping");
    let mut stream = std::net::TcpStream::connect(relay.addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"hello\n")?;
    let mut back = [0u8; 6];
    stream.read_exact(&mut back)?;
    assert_eq!(&back, b"hello\n");
    let c = relay.wait_until(
        Duration::from_secs(3),
        |c| c.udp_forwarded == [1, 1] && c.tcp_forwarded == [6, 6],
        "forwarded",
    )?;
    assert_eq!(c.dropped(), 0);

    // Paused: nothing comes back. The datagram is dropped and counted;
    // the TCP bytes are held, not read, and the stream is counted held.
    relay.pause()?;
    client.send_to(b"lost", relay.addr)?;
    stream.write_all(b"hold!\n")?;
    assert!(
        client.recv_from(&mut buf).is_err(),
        "a datagram crossed a paused relay"
    );
    assert!(
        stream.read(&mut back).is_err(),
        "bytes crossed a paused relay"
    );
    let c = relay.wait_until(
        Duration::from_secs(3),
        |c| c.udp_dropped[0] == 1 && c.tcp_held[0] > 0,
        "paused",
    )?;
    assert_eq!(c.udp_forwarded, [1, 1]);
    assert_eq!(c.tcp_forwarded, [6, 6], "bytes crossed a paused relay");

    // Resumed: the same sockets carry traffic again, and the held bytes
    // arrive first, in order, so a record stream resumes where it stopped.
    relay.resume()?;
    client.send_to(b"back", relay.addr)?;
    let (n, _) = client.recv_from(&mut buf)?;
    assert_eq!(&buf[..n], b"back");
    stream.read_exact(&mut back)?;
    assert_eq!(&back, b"hold!\n", "the held bytes were lost");
    stream.write_all(b"again\n")?;
    stream.read_exact(&mut back)?;
    assert_eq!(&back, b"again\n");
    let c = relay.wait_until(
        Duration::from_secs(3),
        |c| c.udp_forwarded == [2, 2] && c.tcp_forwarded == [18, 18],
        "forwarded again",
    )?;
    assert_eq!(c.udp_dropped, [1, 0]);
    assert!(c.tcp_held[0] > 0 && c.tcp_held[1] == 0, "{c:?}");
    relay.stop();
    Ok(())
}
