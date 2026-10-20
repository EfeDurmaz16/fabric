# ADR 0013: The network protocol as built

Status: accepted, 2026-10-20

Supersedes six parts of ADR 0005: the `Transport` trait and where iroh would plug in, the message shapes under Framing, the 1 MiB control frame figure, the multiplexer header under the TCP+TLS fallback, the dependency list under Consequences, and the open question on `rcgen` and `ed25519-dalek`. Everything else in ADR 0005 stands, including chunks that travel as zstd frames bounded at 8 MiB decompressed.

## Context

ADR 0005 sketched the transport trait and the message vocabulary before any code existed. Building `fabric-net` (PR #5) and the reviews of it changed both. ADR 0009 says an ADR is superseded, not silently contradicted, so this one records what the code does and why.

## Decision

### Where the transport types live, and what the node crate holds

`Transport`, `Connection` and `Stream` live in `fabric-net`, not `fabric-core`. A `Stream` is a tokio `AsyncRead` and `AsyncWrite`, and `fabric-core` does not depend on tokio.

The node crate holds an `Endpoint`, not a `Box<dyn Transport>`. `Endpoint` binds both wires on one port, dials QUIC first and falls back to TCP with TLS, resolves names and walks a peer's address list. `Transport` is the interface of one wire: `QuicTransport` and `TcpTlsTransport` implement it, `Endpoint` holds both by value, and the loopback tests drive each wire through it. So the place to swap the whole network layer is `Endpoint`. If own QUIC plus pairing fails the gate for network reasons, an iroh crate replaces `Endpoint` in the node crate; the core does not change. This replaces ADR 0005's sentence that an iroh implementation would sit behind the `Transport` trait: iroh's endpoint dials by node id over its own paths, so it replaces the endpoint, not one wire.

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

- `connect` takes one address and gives up after the wire's dial timeout, 10 s unless set, on both wires alike. Trying several addresses, resolving names and falling back from QUIC to TCP is the job of `Endpoint::dial_any`, so each wire stays a plain dialer and the fallback rule lives in one place. A fallback reports QUIC's failure as a `TransportError`; the connection's `kind` says which wire served.
- `accept` returns the connection alone. The peer's id is `Connection::peer`, so the fact has one home.
- `kind` and `remote_addr` exist because the ledger records which wire served a resume and the peer book records where a peer was last seen. `close` ends a connection at once; dropping the handle keeps it open while any stream on it lives, on both wires. A stream dropped while a write waits on flow control is cut off, not finished: QUIC resets it, and the TCP multiplexer drops the bytes not yet sent and sends its Fin at once, so a reply abandoned after `REPLY_STALL_TIMEOUT` frees its stream on both wires.
- A transport accepts in the background: every handshake runs in its own task under a 10 s deadline, at most 32 at once and at most 4 from one source, and `accept` reads a queue of finished connections. A source is an IPv4 address or an IPv6 /64, since one IPv6 host is given a whole /64. A TCP socket that sends no byte within 2 s is closed, and QUIC answers an address it has not validated with a Retry before any handshake, so neither a silent host nor a spoofed source can hold the slots a paired peer needs. Racing two transports in `select!` loses nothing.

### Requests, not a message stream

ADR 0005 described one stream of `Message = Hello | Job { ... }`. The code uses one stream per request instead:

```text
Request  = { id: RequestId, budget_ms: u32, body: RequestBody }
Response = { id: RequestId, body: ResponseBody }
```

A slow answer then never holds up another, which per-request streams give for free on QUIC and the multiplexer gives on TCP. `budget_ms` is the time the caller will still wait, and the caller's own deadline fires whatever the peer does. Past the budget the callee answers `Fault::Deadline`, which the caller sees as `RpcError::Deadline`, the same as its own timer. A read stops there. A per-job request does not: it can write ledger records, so it runs to its end in its own task and only the answer races the budget. The caller treats a deadline as an unknown outcome and asks again, with `MoveStatus` for a move. A chunk fetch is bounded by time without progress instead, so a slow link that keeps moving is never cut.

Callers and services see one typed method per request, per-job requests included: `Session::pull`, `append`, `move_request`, `offer`, `move_status`, `move_abort`, `want_snapshot`, `want_chunks` and `ping`, and the same per-job methods plus `manifest` and `chunk` on `Service`. Each per-job request has its own answer type inside `Fenced<T>`, which adds the two epoch verdicts every per-job request can get. On the wire the answers stay one flat `JobResponse`; the rpc layer maps each typed answer onto it and back, and a caller refuses an answer of a kind its request does not take as `UnexpectedResponse`. It also refuses, as `BadVerdict`, an epoch verdict whose `yours` is not the epoch it sent, a `StaleEpoch` whose `current` is not newer, or an `AheadOfUs` whose `current` is not older, and, as `WrongMove`, a `move_status` answer of `Undecided` for another move than the one asked. So neither side can pair a request with an answer of the wrong kind. `Session::offer` takes the `MoveOffer` and the epoch and builds the token from the offer's job, so the two cannot name different jobs. Each service method answers with a `Fault` when it cannot serve, so a store that fails to read a chunk says `Internal`, never `UnknownChunk`. `Hello` and `Ping` are answered by the rpc layer and never reach a service. A session builds its own `Hello` from the local node id and software version and reads the clock as it sends it. A serving session gives a peer 30 s to send a whole request on a stream it opened, so a request that stops halfway gives its stream back. A reply that waits 30 s with no byte taken by the peer ends its stream the same way; only time spent waiting on flow control counts, never the service's work. It runs at most 16 per-job requests at once, counting work that outlived its caller's budget, and sends at most 4 chunk batches at once, so a paired peer cannot pile up ledger work or pin a chunk frame per stream; a request over either cap waits for its budget and is answered `Deadline`.

### The message set

```text
RequestBody  = Hello(Hello) | Ping
             | Job { token: FencingToken, body: JobRequest }
             | WantSnapshot { snapshot } | WantChunks(ChunkBatch)
JobRequest   = Pull { after: Option<RecordHash> } | Append { records: RecordPage } | MoveRequest
             | MoveOffer(OfferBody) | MoveStatus { move_id } | MoveAbort { move_id }
ResponseBody = Hello(Hello) | Pong | Job(JobResponse)
             | ManifestChunks { len, ids } | Chunk { id, zstd } | ChunksDone | Fault(Fault)
JobResponse  = Records(Pulled) | Accepted { head } | Rejected { head }
             | StaleEpoch { yours, current } | AheadOfUs { yours, current }
             | MoveStarted { move_id } | MoveDecision { record: RecordBytes } | MoveUndecided { move_id }
             | SnapshotIncomplete { snapshot }
Pulled       = UpToDate | Last(RecordPage) | More(RecordPage) | NotInChain { head }
Hello        = { node, protocol_version, fabric_version, clock_ms }
OfferBody    = { move_id, head, snapshot }
```

The typed answers above the wire, each inside `Fenced<T> = Answer(T) | StaleEpoch | AheadOfUs`:

```text
Pull        -> Pulled
Append      -> Appended      = Accepted { head } | Rejected { head }
MoveRequest -> MoveRequested = Started { move_id } | Rejected { head }
MoveOffer   -> Offered       = Decided { record } | Rejected { head } | SnapshotIncomplete { snapshot }
MoveStatus  -> MoveState     = Decided { record } | Undecided { move_id }
MoveAbort   -> RecordBytes, the decision record
```

- `Hello` gains `protocol_version`; a mismatch ends the session at the handshake. Skew is the peer's clock against this node's when the hello arrives.
- `Pull` and `Records` let a node fetch the records after its head. A `Pull` with no `after` asks from the job's first record, `Created`, which has no record before it, so a node that holds none of a job's records can still fetch them. `NotInChain { head }` says the asked record is not in the receiver's chain: the two ledgers forked, and the sender runs the ADR 0003 fork rule instead of reading a malformed-request fault. `Pulled` says whether more records follow, and a page is never empty, so a page with more after it always has a last record to pull after, and an `Append` always carries a record. `AheadOfUs` is the mirror of `StaleEpoch`: the sender's epoch is newer, so the receiver must fetch first. `MoveStarted` and `SnapshotIncomplete` are the owner's and the target's answers in the ADR 0003 move.
- `OfferBody` is a `MoveOffer` without its job id. The fencing token carries the job id, so the id has one home on the wire and a message cannot name one job in its token and another in its offer. The receiver rebuilds the `MoveOffer` from the token.
- `Fault` says a request was understood and could not be served: unknown job, snapshot or chunk, budget spent, bad request, internal failure.
- A `ChunkBatch` names 1 to 256 distinct ids; a repeated id is refused at decode, so the missing set stays exact.

### Every bound is parsed, and the largest message fits its frame

- A `RecordBytes` is 1 to `MAX_RECORD_LEN` (1 MiB) bytes, the bound of ADR 0002. Records travel in a `RecordPage` of at least one record, whose cost, each record's length plus 3 bytes for its length prefix, is at most `MAX_RECORD_PAGE`: one record of the largest size, or many small ones. A sender fills a page until it refuses the next record, and `Records` says whether more follow, so a long ledger crosses the wire in pieces.
- The control frame cap is `MAX_RECORD_PAGE` plus 1 KiB for the envelope, 1 MiB and 1,027 bytes in all, not ADR 0005's 1 MiB: a record may be as long as the ledger allows, so the cap has to hold it with its envelope. A test encodes the largest `Append` and `Records` and checks both fit.
- `ManifestChunks` holds a length of 1 to `MAX_MANIFEST_LEN` (64 MiB, ADR 0004) and a chunk count that length allows, at least one per 8 MiB and at most one per byte. The count is also at most `MAX_MANIFEST_CHUNKS`, the ids that fit one control frame beside the envelope, 32,768, so a manifest cut into chunks of 2 KiB on average still fits and the server never fails to write the answer. All of it is checked when the message is built and decoded, before any chunk is fetched.
- The 8 MiB chunk bound is `MAX_CHUNK_LEN` in `fabric_core::snapshot`, next to `MAX_MANIFEST_LEN`. The store cuts objects to it and the network checks arriving chunks against it, so the two cannot drift apart.

### Chunks travel as stored

A chunk travels as the zstd frame the store keeps (ADR 0004), so the server sends it without decompressing and a `node_modules` tree crosses the link compressed. The receiver checks the size the frame declares before it decompresses anything, decompresses into at most 8 MiB, and keeps the chunk only if those bytes hash to the id it asked for. It gets both the frame, which a store can keep as is, and the bytes. A bulk frame holds zstd's worst case for an 8 MiB chunk, 8 MiB and 32 KiB, plus 1 KiB of envelope. This is what ADR 0005 specified.

### Why snapshot and chunk fetches carry no fencing token

Per-job messages carry `FencingToken { job_id, epoch }`, and the receiver runs the ADR 0003 check before it reads the body. `WantSnapshot` and `WantChunks` sit outside the job envelope on purpose. A manifest and a chunk are named by the blake3 hash of their bytes (ADR 0004): the same id is the same bytes whatever epoch asks, the requester verifies the hash before it keeps anything, and serving a read changes no ownership. Fencing guards decisions; it has nothing to guard on an immutable read. A node serves chunks only to paired peers, so the trust rule still applies.

### The TCP multiplexer

Each frame is `u32 stream id | u8 kind | u32 length | payload`, little endian. ADR 0005 had no kind byte; five kinds need it:

- `Data` carries up to 64 KiB of a stream's bytes.
- `Fin` ends one direction.
- `Reset` says this side's reader is gone, so the peer stops sending and its writer fails, as QUIC's `STOP_SENDING` does. It goes out when the reader is dropped, not when the peer next sends, because a peer out of credit sends nothing more and would wait for credit forever.
- `Credit` hands the peer more credit on one stream, a `u32` byte count.
- `KeepAlive` names no stream and says the sender is alive.

The dialer opens odd ids and the acceptor even ids. A stream takes its id when its first frame is queued, under one lock, so each side's ids reach the wire in rising order: a peer id above every one seen so far is a new stream, and any other id without an open stream is closed and its frames are dropped. A peer may hold 100 streams open, quinn's default; one more is refused with a `Reset` and a `Fin`, so its opener's writer fails and its reader sees the end. This side waits in `open` rather than go over the peer's limit, and a stream it opened queues its `Fin` before its slot is free, so the peer always sees the old stream end before the next one starts.

The connection's reader waits on nothing but the wire. The frames it sends itself, a `Reset` or a refusal, go through a control queue that the writer drains before the data queue, and the reader queues them without waiting. A full control queue means the peer makes this side reset streams while it reads nothing, and ends the connection. So two peers whose writers both wait on a full wire still read, and both drain.

Each direction of a stream has its own credit, as each QUIC stream has its own window. A sender starts with 1 MiB and spends it on `Data`; the receiver buffers exactly that much and returns credit each time its reader has taken 256 KiB. The connection's reader never waits on one stream, so a stream nobody reads stops only its own sender, and a caller may read its streams one after the other on either wire. A peer that sends past its credit, or grants more than a buffer holds, ends the connection.

As on QUIC, a side that has written nothing for 10 s sends `KeepAlive`, and a side that has read no byte for 30 s ends the connection, so a peer that slept or changed networks does not leave a half-open connection. The writer flushes TLS whenever its queue runs empty, so a stream's last bytes never wait in the TLS buffer for a frame that does not come.

### Versions

Three numbers version the protocol, each for one layer:

- The ALPN `fabric/1` gates the transport: the handshake, and on TCP the multiplexer frame format. A change to either bumps it, and the handshake fails between versions. A peer that selects no ALPN is refused too: rustls refuses it for QUIC, and the TCP wire checks the selected protocol on both sides before the multiplexer starts, since TLS over TCP lets a client that offers none finish.
- `PROTOCOL_VERSION` in `Hello` gates the message vocabulary. A change to any request, response or bound bumps it, and two nodes on different versions refuse each other at `Hello` instead of misreading frames.
- `FRAME_VERSION`, the byte after each frame's length, names the layout of what follows the length, and changes when that layout does. It cannot version the length prefix itself: a reader parses the length before it sees the byte. A new length format bumps the ALPN instead.

### Which side dials is not recorded yet

ADR 0005 says a pair records which side can dial. The peer book does not record it yet: it keeps the addresses the user pasted at pairing, newest first, and an empty list means only that no address is known. `PeerBook::seen` records only when a connection completed, never its remote address: an accepted connection comes from the dialer's ephemeral port, or from a NAT, and eight of those would push the pasted address out of the list. The node crate is the first code that decides whether to dial, so it records the direction and the addresses it dialed; until then nothing reads the list as a dial rule.

### Trust details the code added

- TLS session resumption is off on both sides. A resumed TLS 1.3 handshake restores the peer's certificate from the earlier session without asking the verifier, so a node unpaired since then would get back in.
- The verifier reads only the peer book in memory, because it runs on the thread that drives the handshake and, for QUIC, every connection of the endpoint. The accept path re-reads the book file on a blocking thread before each handshake and again after it, so a pairing made by `fabric node pair` in another process counts at the next handshake, and an unpair made during a handshake still holds. A re-read parses the file again whenever its bytes changed. A length or a modified time can stay the same across an edit made in place; the bytes cannot.
- An unpair ends live connections too. `Session::serve` checks the decision before each request and re-reads it every 5 s, and closes the connection of a peer that is no longer paired. Keep-alives would otherwise hold an unpaired node's connection open for as long as it likes.
- A book file that cannot be read or does not parse pairs no one until it does. A hand edit that revokes a node and slips in a typo never leaves that node trusted. A book line holds at most 8 distinct addresses and a node appears on one line only, the same bounds the code keeps when it writes.
- A dial whose peer signed the handshake with a key its certificate does not name is a forgery and fails at once. Any other refusal, such as another node answering at a stale address, moves on to the next address. On one address the endpoint never tries the other wire after a refusal: the node there is the same on both.

### Dependencies

`ed25519-dalek` is not used: `rcgen` on `ring` generates the ed25519 key, writes it as PKCS#8 and signs the certificate, so one ed25519 implementation is in the tree. `zstd` 0.14 checks and decompresses arriving chunks, the same crate and version the store uses. `mdns-sd` is not used in this phase; LAN address refresh is not built.

## Alternatives considered

1. **Keep ADR 0005's single message stream with a job envelope.** Correlating answers, deadlines and head-of-line blocking would all have to be built on top of one ordered stream. Per-request streams get them from the transport. Rejected.
2. **Edit ADR 0005 in place.** ADR 0009 keeps accepted ADRs as they were and records a change in a new one. Rejected.
3. **Put the transport traits in `fabric-core`.** The core would depend on tokio for the stream types. Rejected; only `BoxFuture`, which needs nothing but std, lives there.
4. **Fence chunk fetches by epoch.** It would refuse a valid read from a node one epoch behind, which is exactly the node catching up. Rejected for content-addressed reads.
5. **A dyn trait over `Endpoint`, so the node crate holds a `Box<dyn ...>`.** It would have one implementation until iroh is tried, and iroh's endpoint has its own shape. Rejected for now; the node crate names `Endpoint`, and swapping it is one type change there.
6. **Send chunks uncompressed.** Simpler to serve and to check, but the server would decompress and the client recompress every chunk, and the gate's `node_modules` would cross the link several times larger. Rejected.
7. **TCP keep-alive on the socket instead of a `KeepAlive` frame.** It needs one more dependency to set the timers and cannot be tested on loopback, where the kernel answers for a peer that has stopped reading. Rejected.

## Consequences

- ADR 0005's sections on the trait, the message shapes, the control frame figure, the multiplexer header, its dependency list and its `rcgen` question are history; this ADR and `crates/fabric-net/src/wire.rs` are the reference. The rest of ADR 0005 stands.
- `docs/net.md` describes how to run the layer; it follows this ADR.

## Open questions

- How the peer book records which side dials, and the addresses this node dialed: a flag per peer, a list of dialed addresses kept apart from the pasted ones, or both. Decide with the node crate.
- Should the node crate own the `Service` implementation, or should the store answer `manifest` and `chunk` directly? Decide with the node crate.
