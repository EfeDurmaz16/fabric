# ADR 0005: Transport and node identity

Status: accepted, 2026-10-19

## Context

Nodes must find each other, prove who they are, and move ledgers and chunks between them. Efe's constraint: no vendor dependency in networking. Tailscale may be present and is a fine network to run over, but Fabric must work on a plain LAN. iroh is a serious Rust option and the research lists it as the reuse candidate (`2026-10-19-competitor-landscape.md`, section 8.1); the same research notes n0 needed four years and 65 pre-releases to reach 1.0, which is the reason to keep it behind a trait rather than in the core.

The traffic is: small control messages (ledger events, lease claims, manifest fetches) and bulk chunk transfer that must survive a dropped connection.

## Decision

### Node identity

- A node has one ed25519 keypair, generated on first run, stored at `<fabric-data>/node.key` with mode 0600. The public key is the `NodeId`.
- The text form is `fab1` followed by base32 of the 32 bytes, so ids are easy to read aloud and paste.
- Nodes never share private keys. A new machine is a new node.

### The `Transport` trait

Defined in `fabric-core`, implemented in `fabric-net`.

```text
trait Transport {
    async fn connect(&self, peer: NodeId, addrs: &[SocketAddr]) -> Result<Connection, TransportError>;
    async fn accept(&self) -> Result<(NodeId, Connection), TransportError>;
}
trait Connection {
    async fn open(&self) -> Result<Stream, TransportError>;     bidirectional byte stream
    async fn next(&self) -> Result<Stream, TransportError>;     peer-opened stream
    fn peer(&self) -> NodeId;
}
```

The core talks to a `Connection` and `Stream` and nothing else. Everything below the trait is replaceable.

### QUIC first

`quinn` 0.11 over `rustls` 0.23.

- Each node generates a self-signed certificate with `rcgen` from its ed25519 key, so the certificate's public key is the node id.
- The client verifier ignores names and CAs and checks exactly one thing: the certificate's public key equals the `NodeId` it dialed. The server requires a client certificate and checks the same.
- Wrong key: connection refused before any application byte.
- QUIC gives multiplexed streams (control and bulk do not block each other), 0-RTT reconnect, and migration across address changes (a laptop moving from Wi-Fi to tethering keeps its connection).

### TCP+TLS fallback

Some networks drop or rate-limit UDP. The same `rustls` config runs over a `tokio` TCP stream with a trivial stream multiplexer (u32 stream id, u32 length, payload). It is slower and has no migration. A node tries QUIC first and falls back after a short timeout; the ledger records which transport served a resume so slow paths are visible.

### Framing

Every message on a control stream is `u32 length | u8 version | postcard(Message)`. Every `Message` carries `job_id` and `epoch` (ADR 0003). The first message on a new connection is `Hello { node, fabric_version, clock_ms }` in both directions; a clock difference over 10 seconds is logged and reported.

### Blob transfer

Streams are per request. A fetch is:

1. `WantSnapshot { snapshot }` on a control stream; peer replies with the manifest.
2. Requester computes the missing chunk set from its catalog and sends `WantChunks { ids: [...] }` in batches of up to 256 ids on a bulk stream.
3. Peer streams `Chunk { id, bytes }` records. Each is verified by blake3 on arrival and written with the publish discipline of ADR 0004.
4. On any failure the requester recomputes the missing set from its catalog and sends a new batch. Nothing is tracked in memory that is not also in the catalog.

A chunk is never requested by offset; only by id. There is no partial-chunk resume because chunks are small.

### Pairing first, discovery second

- Trust is established only by pairing: `fabric node pair fab1...@192.168.1.20:7777` on one node, and the same with the other id on the other. The id comes from an out-of-band channel (the user reads it off the other screen). Pairing writes the peer's id and last known addresses to `<fabric-data>/peers`.
- mDNS (`mdns-sd`) is optional and advertises only `_fabric._udp` with the node id in the TXT record. It is used to refresh the address of an already paired node on the LAN. A node seen by mDNS but not paired is listed as "unpaired" and never connected to.
- Over Tailscale the addresses are stable `100.x` or MagicDNS names. Fabric does not know or care; the user pastes them at pairing.

### No vendor dependency

Fabric runs with no account, no relay, no coordination server. Two nodes that can reach each other's address can work. Nodes that cannot (both behind symmetric NAT, no tailnet) cannot, and v0 says so instead of pretending.

## Alternatives considered

1. **iroh.** Apache-2.0 and MIT, stable wire since 2026-06, QUIC, hole punching, relays, `iroh-blobs` for BLAKE3 transfer. It would delete most of `fabric-net`. Costs: a large dependency tree, a public relay network by default (self-hostable), and a design that assumes iroh's endpoint model everywhere. Decision: not in v0, but the `Transport` trait is shaped so an `iroh` implementation is one crate with no core change. If own QUIC plus pairing fails the two-Mac gate for network reasons, iroh is the first thing to try.
2. **Tailscale as the transport** (`tsnet`, `libtailscale`, `tailscale-rs`). Go-only or pre-alpha from Rust; also makes Fabric unusable without a Tailscale account. Rejected as a dependency; welcomed as a network.
3. **SSH as the transport.** Works everywhere, but every user already has it and the research shows the manual baseline is `git push; ssh; pnpm i; claude --resume`. Fabric needs multiplexed, resumable, id-authenticated streams; shelling out to `ssh` for that is fragile. Rejected.
4. **HTTP/2 with mTLS.** Would work, more ceremony, no migration, and pulls in an HTTP stack for what is a byte protocol. Rejected.
5. **Noise protocol over TCP instead of TLS.** Simpler handshake, no certificates. `rustls` with pinned keys is well trodden and gives QUIC for free. Rejected.
6. **A hub or relay node.** Reintroduces the "hub must stay awake" failure the research assigns to T3 Code and bb. Rejected.

## Consequences

- One keypair per node, one line to pair. No PKI, no accounts.
- The only network-level trust decision in the system is "is this key one I paired with". Everything else is fenced by epoch at the job level.
- Dependencies in `fabric-net`: `quinn`, `rustls`, `rcgen`, `ed25519-dalek`, `tokio`, `postcard`, `mdns-sd`. All had releases in the last three months as of 2026-10-19.
- Integration tests run two nodes in one process on loopback with real QUIC and real TLS, and one test forces the TCP fallback by binding a UDP socket the server cannot use.

## Open questions

- Key rotation: not in v0. A compromised node key means re-pair. Document it.
- Should pairing be mutual automatically (one side pairs, the other confirms over the connection)? Probably yes; design when writing the CLI.
- Does `rcgen` accept an `ed25519-dalek` 3.x key directly or through `pkcs8`? Check at implementation time; it affects only the certificate builder.
