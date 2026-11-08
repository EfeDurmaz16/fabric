# The network layer

`crates/fabric-net` implements ADR 0005 as ADR 0013 records it. This page says
how the layer is used on the networks Efe has and what it refuses. ADR 0013
holds the mechanics: the message set, the bounds, the multiplexer and the
timers.

The node daemon, `fabric serve` (`crates/fabricd`), holds one `Endpoint` and
serves the requests of paired peers; the `fabric` CLI pairs nodes with
`fabric node add` and checks a peer with `fabric node ping`. A program that
embeds the layer pairs through `Peers::update` with `PeerBook::pair`, and the
loopback tests run two `Endpoint`s in one process.

## What a node is

A node is an ed25519 keypair in `~/.fabric/node.key` (or under
`$FABRIC_HOME`), mode 0600, written
once. The node id is the public key, printed as `fab1` plus 52 base32
characters. Two machines never share a key; a new machine is a new node.

The key signs a self-signed certificate. The certificate is only the carrier:
a client accepts a server only if the certificate's key equals the id it
dialed, a server accepts a client only if the certificate's key is in the
peer book, and both check the handshake signature against that key itself.
No CA, no name, no date. TLS 1.3 only, ed25519 only, 0-RTT off, session
resumption off: a resumed handshake would skip the pairing check, so every
handshake is a full one.

## Pairing

Trust is the peer book, one line per peer:

```text
fab1...  192.168.1.20:7777,100.101.102.103:7777  1760000000000
```

Pairing v0 is out of band. Each node prints its id with `fabric node id`. On
the side that can dial, pair the other with its id and an address it reaches
today: `fabric node add fab1<air-id> <air-address>:7777`. On the side that
cannot dial, such as a node behind a NAT, pair with the id alone:
`fabric node add fab1<pro-id>`; that peer reaches it over the connection it
opens. `fabric node ls` lists the book. An address takes an IP address or a
name; a name is resolved at dial time, and its first 4 distinct addresses are
dialed. A node that is not in the book is refused inside the TLS
handshake and never reaches application code, listening or not.

An `Endpoint` re-reads the book before each handshake, so a pairing written
by another process while it runs counts at the next connection. An unpair
also ends a connection that is already open, within 5 s. A book that does
not parse pairs no one until it is fixed, and the error names the line. A
line holds at most 8 distinct addresses, and a node appears on one line.
The addresses are the ones pasted at pairing; a connection updates only the
last-seen time, never the list. Every save writes the book with mode 0600,
and a book that a group or other users can write pairs no one until it is
`chmod 600` again.

A short pairing code, where one side shows a few words and the other types
them to fetch and confirm the id over the connection, is a follow-up. It
needs a key exchange that this phase does not have.

## Versions

Two nodes must run the same protocol version, which `Hello` carries; a node of
another version is refused at the handshake of the session, with both versions
named, instead of misreading a request. Version 4 added the question a push
asks its target for the facts of the job's harness (ADR 0013, amended
2026-10-27), so a node updated past it pairs only with nodes updated too.

Version 4 has not shipped, and M5 adds two more messages to it (ADR 0026,
ADR 0027). `Advertise` asks a peer what it offers for the placement of one
managed job and is answered with `Advert`: platform, harnesses with versions,
path layout, load and free disk, warmth for the job, the time measured, and
the job's secrets the peer cannot resolve. `Facade` carries one facade
message, JSON of at most 1 MiB that the link does not read, to the peer's
managed authority, and is answered with `Facade`, the authority's answer of at
most 16 MiB, unchanged (docs/facade-protocol.md). A facade message runs under
the per-peer job work cap like a per-job message, so a watch holds one slot
for at most its wait, and its answer alone is read under the facade's own
frame bound; every other control message keeps the control frame's. A peer
that serves neither answers a fault, which the asking node records as that
peer's exclusion or answers its client as `unavailable`.

The launch path (docs/managed-jobs.md, "Launch path (M5, as built)") adds
four more messages to version 4, still unshipped. `Launch` carries a grant
from a job's authority to the node it chose: the token (job, attempt,
generation), the intent, the source, the policy, the checkpoint to start
from and the node whose store holds it, the build and test tasks with
their programs, and for a planned handoff the continuation and its work
package; the node answers `Accepted` or `Refused` at once and runs the
attempt after. `Instruct` asks the node that runs an attempt to spawn a
task, to quiesce and cut (then resume or end), or to stop. `Report`
carries one observation of an attempt back to the authority under the
grant's token and is answered `Recorded` with the job's revision, or
`Refused`; a token whose generation is not the job's is refused
`StaleGeneration` and the node stops the attempt. `Heartbeat` names every
grant a node runs, every two seconds while it runs one, and is answered
with a verdict per grant (`Live`, `Stale`, `Unknown`); it writes nothing.
All four fit the control frame (a launch's package is at most 256 KiB)
and run under the per-peer job work cap. The shapes are
`fabric_core::managed::launch`; the receiving side checks every bound
again after it decodes.

## Launch path fixtures

[`protocol/net/v4`](../protocol/net/v4) holds one case per message form
and a manifest, as the facade's fixtures do (docs/facade-protocol.md,
"Fixtures"). A case is `{ "case", "direction", "message", "expect" }`:
`direction` is `authority_to_node` or `node_to_authority`; `message` is
the message's fields in the client API's text forms (ids as text,
counters as decimal strings, a `type` naming the request); `expect` is
`{ "outcome": "accept" }` or `{ "outcome": "reject", "reason": ... }`.
The pin test, `crates/fabric-net/tests/launch_fixtures.rs`, checks every
file against `manifest.sha256`, that none is missing or extra, and that
the manifest's own sha256 is the one pinned there; then it builds each
accepted case's message with the Rust constructors, encodes it as a
request in a control frame, decodes it and compares, and checks that each
rejected case is refused by the check its `reason` names
(`Program::check`, `Report::check`, the list bounds). The manifest's
sha256 as of this contract's commit is
`972ed115913ac8cd4ca7aeb6e48923c5e0e82dcbcaa3839d502c5be55882b7aa`; the
pin test carries the same constant, and it moves only with the fixtures,
in the same commit.

As built: the four messages are `RequestBody::{Launch, Instruct, Report,
Heartbeat}`, answered by the `ResponseBody` of the same name, and served
under the per-peer job work cap like a facade message: the work runs to
its end, only the answer races the budget. The list bounds
(`MAX_TASK_LAUNCHES` tasks in a launch, `MAX_HEARTBEAT_ATTEMPTS` grants in
a heartbeat) are `RequestBody::check_bounds`, run when a request is
parsed (`Request::from_control_payload`) and before `Session::launch` or
`Session::heartbeat` sends one. The checks that answer a refusal rather
than a fault are the receiver's: `LaunchRequest::check` (programs,
`snapshot_at` with a checkpoint, a package exactly with a continuation,
the package's job and size) and `Report::check`. `Service::{launch,
instruct, report, heartbeat}` refuse by default with `Fault::BadRequest`.

```sh
cd protocol/net/v4
find . -type f ! -name manifest.sha256 | sed 's|^\./||' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256
shasum -a 256 manifest.sha256
```

## Wires

Both transports listen on one port, and the node holds them as one
`Endpoint`. A dial tries QUIC for 3 s, then TCP with TLS. A fallback says
how QUIC failed, and the connection says which wire served it, so a slow
path is never silent. A forged handshake fails the dial at once. Another
node answering at one of the peer's addresses, which a stale address can do,
only moves the dial on to the next address.

A host that opens sockets and stays silent, or a spoofed source, cannot
hold up a paired peer: handshakes run in the background with a time limit,
a cap in all and a cap per source.

A paired peer is bounded too, over all its connections at once: a node runs
at most 16 of its per-job requests and sends it at most 4 chunk batches at
a time, and holds at most about 16 MiB of its requests, read or being read.
A request with no room waits for at most 30 s, unread at the peer past what
the wire takes before anyone reads: its first 16 KiB on TCP, and on QUIC
whatever fits the connection's receive window of about 6 MB, which bounds
all of a peer's unread bytes on it (ADR 0013, amended 2026-10-27 and
2026-10-29).

## On the gate network

Measured 2026-10-19: the Air has a public address with the firewall off; the
Pro is behind NAT; UDP and TCP both arrive from the Pro at the Air.

- Pro to Air over the public address: pair with the Air's public IP, port
  7777. The Pro dials; QUIC works direct, TCP is the fallback. Every message
  flows both ways over the one connection, so the Air can push and pull once
  the Pro connected.
- Over Tailscale: start it on both, pair with the `100.x` address or the
  MagicDNS name instead. Nothing in Fabric changes; the pinned handshake runs
  inside the tailnet's encryption. Fabric does not depend on Tailscale and
  does not read its state.
- On a LAN: pair with the LAN address. mDNS discovery, which ADR 0005
  allows only to refresh the address of a node that is already paired, is
  not built yet.
- Two nodes behind symmetric NAT with no tailnet cannot reach each other.
  v0 says so instead of pretending; iroh is the first thing to try if the
  gate fails for network reasons (ADR 0005, alternative 1).

## Tests

`cargo test -p fabric-net`. `tests/loopback.rs` runs two nodes in one
process on real QUIC and real TLS over TCP; each module's own tests cover
its parser, bounds and failure modes, and the crash tests kill a child
process while it writes the node key or the peer book.

## Not in this phase

Key rotation (a compromised key means re-pair), a pairing code, mutual
pairing over the connection, mDNS address refresh on a LAN, a hub or
relay, and NAT traversal.
