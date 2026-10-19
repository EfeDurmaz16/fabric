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
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

trait Transport: Send + Sync {
    fn connect(&self, peer: NodeId, addrs: &[SocketAddr]) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>>;
    fn accept(&self) -> BoxFuture<'_, Result<(NodeId, Box<dyn Connection>), TransportError>>;
}
trait Connection: Send + Sync {
    fn open(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;     bidirectional byte stream
    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;     peer-opened stream
    fn peer(&self) -> NodeId;
}
```

The transport is chosen at runtime (QUIC, then the TCP fallback), so the core holds a `Box<dyn Transport>` and spawns its futures on tokio. On the pinned toolchain a trait with native `async fn` is not dyn compatible, and return-type notation for `Send` bounds is unstable, so the methods return a boxed `Send` future instead. `BoxFuture` is defined once in `fabric-core` and is the shape of every trait the core dispatches dynamically (`Harness` in ADR 0006 uses the same one). One allocation per call is nothing next to a network round trip.

The core talks to a `Connection` and `Stream` and nothing else. Everything below the trait is replaceable.

### QUIC first

`quinn` 0.11 over `rustls` 0.23.

- Each node generates a self-signed certificate with `rcgen` from its ed25519 key, so the certificate's public key is the node id.
- TLS 1.3 only, Ed25519 only. Both verifiers are custom `rustls` verifiers that ignore names, CAs and validity dates. A certificate is public, so matching its key is not authentication by itself; each verifier also verifies the handshake's `CertificateVerify` signature against that key through `verify_tls13_signature`, which proves the peer holds the private key for the session at hand.
- The client verifier (`ServerCertVerifier`) accepts only if the certificate's public key equals the `NodeId` it dialed and the key is in `<fabric-data>/peers`.
- The server verifier (`ClientCertVerifier`) requires a client certificate and accepts only if its public key is in `<fabric-data>/peers`. An unknown key is rejected inside the handshake; no application byte is exchanged. This is the accept-side half of "pairing is trust": listening does not mean accepting.
- 0-RTT is disabled. Early data is replayable, and the control vocabulary includes claims and appends; nothing in this protocol is worth a replayed claim.
- QUIC gives multiplexed streams (control and bulk do not block each other) and migration across address changes (a laptop moving from Wi-Fi to tethering keeps its connection).

### TCP+TLS fallback

Some networks drop or rate-limit UDP. The same `rustls` config runs over a `tokio` TCP stream with a trivial stream multiplexer (u32 stream id, u32 length, payload). It is slower and has no migration. A node tries QUIC first and falls back after a short timeout; the ledger records which transport served a resume so slow paths are visible.

### Framing

Every message on a control stream is `u32 length | u8 version | postcard(Message)`. A control frame longer than 1 MiB is a protocol error and closes the connection; a bulk `Chunk` frame is bounded by the chunk size cap of ADR 0004 plus framing, and a chunk whose zstd frame declares, or decompresses to, more than 256 KiB is rejected before allocation. A peer cannot make a node allocate more than one frame's worth of memory per stream. Nothing of unbounded size is ever a control message: ledger records are small by construction, and manifests go over the bulk path as chunks (Blob transfer, below). Every `Message` carries `job_id` and `epoch` (ADR 0003). The first message on a new connection is `Hello { node, fabric_version, clock_ms }` in both directions; a clock difference over 10 seconds is logged and reported.

### Blob transfer

Streams are per request. A fetch is:

1. `WantSnapshot { snapshot }` on a control stream; peer replies `ManifestChunks { len, ids }`, the manifest's byte length and ordered chunk list. The manifest itself never travels on a control stream: a manifest with `node_modules` in it is several megabytes and a control frame is capped at 1 MiB. The id list for a 64 MiB manifest is about 32 KiB, so the reply always fits. A `len` above the 64 MiB manifest cap of ADR 0004 is refused before any chunk is requested.
2. Requester fetches the manifest chunks exactly as in steps 3 and 4, assembles them, checks that blake3 of the bytes equals `snapshot`, and decodes under the size cap. Only then does it compute the missing chunk set from its catalog and send `WantChunks { ids: [...] }` in batches of up to 256 ids on a bulk stream.
3. Peer streams `Chunk { id, bytes }` records. Each is verified by blake3 on arrival and written with the publish discipline of ADR 0004.
4. On any failure the requester recomputes the missing set from its catalog and sends a new batch. Nothing is tracked in memory that is not also in the catalog.

A chunk is never requested by offset; only by id. There is no partial-chunk resume because chunks are small. The only size a peer can declare is `len`, and it is checked against the cap and against the assembled bytes, so a hostile peer cannot make a node allocate more than one frame plus one manifest cap.

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
- Integration tests run two nodes in one process on loopback with real QUIC and real TLS, and one test forces the TCP fallback by binding a UDP socket the server cannot use. Two more are named here because they guard the trust rule: an unpaired key is refused at accept with no application byte exchanged, and a dialer presenting a paired node's certificate without its private key fails the handshake.

## Open questions

- Key rotation: not in v0. A compromised node key means re-pair. Document it.
- Should pairing be mutual automatically (one side pairs, the other confirms over the connection)? Probably yes; design when writing the CLI.
- Does `rcgen` accept an `ed25519-dalek` 3.x key directly or through `pkcs8`? Check at implementation time; it affects only the certificate builder.
