# ADR 0013: The network protocol as built

Status: accepted, 2026-10-20

Supersedes three parts of ADR 0005: the `Transport` trait, the message shapes under Framing, and the multiplexer header under the TCP+TLS fallback. Everything else in ADR 0005 stands.

## Context

ADR 0005 sketched the transport trait and the message vocabulary before any code existed. Building `fabric-net` (PR #5) and two reviews of it changed both. ADR 0009 says an ADR is superseded, not silently contradicted, so this one records what the code does and why.

## Decision

### Where the transport types live

`Transport`, `Connection` and `Stream` live in `fabric-net`, not `fabric-core`. A `Stream` is a tokio `AsyncRead` and `AsyncWrite`, and `fabric-core` does not depend on tokio. The node crate holds a `Box<dyn Transport>` as ADR 0005 planned.

`BoxFuture` stays in `fabric-core` and is defined there once. `fabric-net` imports it, and the `Harness` trait of ADR 0006 will use the same type.

### The trait's shape

```text
trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;
    fn local_addr(&self) -> Result<SocketAddr, TransportError>;
    fn connect(&self, peer: NodeId, addr: SocketAddr) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>>;
    fn accept(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>>;
}
trait Connection: Send + Sync {
    fn peer(&self) -> NodeId;
    fn kind(&self) -> TransportKind;
    fn remote_addr(&self) -> SocketAddr;
    fn open(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;
    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;
    fn close(&self);
}
```

- `connect` takes one address. Trying several addresses, resolving names and falling back from QUIC to TCP is the job of `Endpoint::dial_any`, so each wire stays a plain dialer and the fallback rule lives in one place.
- `accept` returns the connection alone. The peer's id is `Connection::peer`, so the fact has one home.
- `kind` and `remote_addr` exist because the ledger records which wire served a resume and the peer book records where a peer was last seen. `close` ends a connection at once; dropping the handle keeps it open while any stream on it lives, on both wires.
- A transport accepts in the background: every handshake runs in its own task under a deadline, at most 32 at once, and `accept` reads a queue of finished connections. A host that opens a socket and stays silent cannot hold up a paired peer, and racing two transports in `select!` loses nothing.

### Requests, not a message stream

ADR 0005 described one stream of `Message = Hello | Job { ... }`. The code uses one stream per request instead:

```text
Request  = { id: RequestId, budget_ms: u32, body: RequestBody }
Response = { id: RequestId, body: ResponseBody }
```

A slow answer then never holds up another, which per-request streams give for free on QUIC and the multiplexer gives on TCP. `budget_ms` is the time the caller will still wait; the callee stops work past it, and the caller's own deadline fires whatever the peer does. A chunk fetch is bounded by time without progress instead, so a slow link that keeps moving is never cut.

Callers and services see one typed method per request (`Session::job`, `want_snapshot`, `want_chunks`, `ping`; `Service::job`, `manifest`, `chunk`), so neither side can pair a request with an answer of the wrong kind. `Hello` and `Ping` are answered by the rpc layer and never reach a service.

### The message set

```text
RequestBody  = Hello(Hello) | Ping
             | Job { token: FencingToken, body: JobRequest }
             | WantSnapshot { snapshot } | WantChunks(ChunkBatch)
JobRequest   = Pull { after } | Append { records } | MoveRequest
             | MoveOffer(MoveOffer) | MoveStatus { move_id } | MoveAbort { move_id }
ResponseBody = Hello(Hello) | Pong | Job(JobResponse)
             | ManifestChunks { len, ids } | Chunk { id, bytes } | ChunksDone | Fault(Fault)
JobResponse  = Records | Accepted { head } | Rejected { head }
             | StaleEpoch { yours, current } | AheadOfUs { yours, current }
             | MoveStarted { move_id } | MoveDecision { record } | MoveUndecided { move_id }
             | SnapshotIncomplete { snapshot }
Hello        = { node, protocol_version, fabric_version, clock_ms }
```

- `Hello` gains `protocol_version`; a mismatch ends the session at the handshake. `clock_ms` is read as the hello is sent, and skew is the peer's clock against this node's when the hello arrives.
- `Pull` and `Records` let a node fetch the records after its head. `AheadOfUs` is the mirror of `StaleEpoch`: the sender's epoch is newer, so the receiver must fetch first. `MoveStarted` and `SnapshotIncomplete` are the owner's and the target's answers in the ADR 0003 move.
- `Fault` says a request was understood and could not be served: unknown job, snapshot or chunk, budget spent, bad request, internal failure.
- A `ChunkBatch` names 1 to 256 distinct ids; a repeated id is refused at decode, so the missing set stays exact.

### Why snapshot and chunk fetches carry no fencing token

Per-job messages carry `FencingToken { job_id, epoch }`, and the receiver runs the ADR 0003 check before it reads the body. `WantSnapshot` and `WantChunks` sit outside the job envelope on purpose. A manifest and a chunk are named by the blake3 hash of their bytes (ADR 0004): the same id is the same bytes whatever epoch asks, the requester verifies the hash before it keeps anything, and serving a read changes no ownership. Fencing guards decisions; it has nothing to guard on an immutable read. A node serves chunks only to paired peers, so the trust rule still applies.

### The TCP multiplexer

Each frame is `u32 stream id | u8 kind | u32 length | payload`, little endian. ADR 0005 had no kind byte; three kinds need it:

- `Data` carries up to 64 KiB of a stream's bytes.
- `Fin` ends one direction.
- `Reset` says this side's reader is gone, so the peer stops sending and its writer fails, as QUIC's `STOP_SENDING` does.

The dialer opens odd ids and the acceptor even ids. A stream takes its id when its first frame is queued, under one lock, so each side's ids reach the wire in rising order: a peer id above every one seen so far is a new stream, and any other id without an open stream is closed and its frames are dropped. A peer may hold 100 streams open, quinn's default; one more is reset, and this side waits in `open` rather than go over the peer's limit.

### Which side dials is not recorded yet

ADR 0005 says a pair records which side can dial. The peer book does not record it yet: it keeps the addresses a peer was reached at, newest first, and an empty list means only that no address is known. `PeerBook::seen` adds the address an accepted connection came from, which for the Pro behind NAT is not an address the Air can dial. The node crate is the first code that decides whether to dial, so it records the direction; until then nothing reads the list as a dial rule.

### Trust details the code added

- TLS session resumption is off on both sides. A resumed TLS 1.3 handshake restores the peer's certificate from the earlier session without asking the verifier, so a node unpaired since then would keep access.
- An accepted connection is checked against the peer book once more after its handshake, so an unpair made during the handshake holds.
- A dial the client verifier refused, a wrong node id or a forged signature, fails at once; the endpoint does not try the other wire or the next address.

### Dependencies

`ed25519-dalek` is not used: `rcgen` on `ring` generates the ed25519 key, writes it as PKCS#8 and signs the certificate, so one ed25519 implementation is in the tree. `mdns-sd` is not used in this phase; LAN address refresh is not built.

## Alternatives considered

1. **Keep ADR 0005's single message stream with a job envelope.** Correlating answers, deadlines and head-of-line blocking would all have to be built on top of one ordered stream. Per-request streams get them from the transport. Rejected.
2. **Edit ADR 0005 in place.** ADR 0009 keeps accepted ADRs as they were and records a change in a new one. Rejected.
3. **Put the transport traits in `fabric-core`.** The core would depend on tokio for the stream types. Rejected; only `BoxFuture`, which needs nothing but std, lives there.
4. **Fence chunk fetches by epoch.** It would refuse a valid read from a node one epoch behind, which is exactly the node catching up. Rejected for content-addressed reads.

## Consequences

- ADR 0005's sections on the trait, the message shapes and the multiplexer header are history; this ADR and `crates/fabric-net/src/wire.rs` are the reference. The rest of ADR 0005 stands.
- A protocol change bumps `PROTOCOL_VERSION`, and two nodes on different versions refuse each other at `Hello` instead of misreading frames.
- `docs/net.md` describes how to run the layer; it follows this ADR.

## Open questions

- How the peer book records which side dials: a flag per peer, or `seen` keeping only addresses this node dialed. Decide with the node crate.
- Should the node crate own the `Service` implementation, or should the store answer `manifest` and `chunk` directly? Decide with the node crate.
- TCP has no keep-alive: a peer that vanishes leaves a connection open until a write fails. QUIC's 30 s idle timeout covers it there. Decide with the node crate whether a session-level ping is enough.
