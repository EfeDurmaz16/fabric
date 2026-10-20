# The network layer

`crates/fabric-net` implements ADR 0005. This page says how to run it on the
networks Efe has, what it refuses, and what is left for later.

## What a node is

A node is an ed25519 keypair in `<fabric-data>/node.key`, mode 0600, written
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

Pairing v0 is out of band. On the Air, read the id off the screen of the Pro
and run `fabric node pair fab1<pro>@<pro-address>:7777`; on the Pro, the same
with the Air's id. A spec takes an IP address or a name; a name is resolved
at dial time. A node that is not in the book is refused inside the TLS
handshake and never reaches application code, listening or not.

A short pairing code, where one side shows a few words and the other types
them to fetch and confirm the id over the connection, is a follow-up. It
needs a key exchange that this phase does not have.

## Wires

Both transports listen on one port. A dial tries QUIC for `QUIC_TIMEOUT`
(3 s), then TCP with TLS, which gets 10 s for the connect and the handshake
together. The fallback is reported with its reason and the ledger will
record which wire served a resume, so a slow path is visible. A dial that
reached the wrong node, or a forgery, fails at once and tries nothing else.

Each transport accepts in the background: every handshake runs in its own
task with a 10 s limit, at most 32 at once, so a host that opens the port
and stays silent does not hold up a paired peer.

- QUIC (`quinn` 0.11 over `rustls` 0.23): multiplexed streams, migration
  across address changes, keep-alive every 10 s, idle timeout 30 s.
- TCP with TLS (`tokio-rustls`): the same verifiers over a TCP stream with a
  small multiplexer (`u32 stream id | u8 kind | u32 length | payload`, kinds
  data, fin and reset). A dropped stream resets the peer's writer, a peer
  may hold 100 streams open, and a connection lives while its handle or a
  stream does, as on QUIC. One stream that nobody reads holds up the
  others once its 1 MiB buffer is full; the protocol reads every stream it
  opens.

ADR 0013 records the trait, the message set and the multiplexer as built.

## Framing and messages

A frame is `u32 length | u8 version | postcard(message)`. A control frame is
at most 1 MiB; a chunk frame at most 8 MiB plus the envelope. A reader
checks the declared length before it allocates, so a peer cannot make a node
hold more than one frame per stream.

Every request runs on its own stream and carries a request number and the
milliseconds the caller still allows. The caller's deadline fires whatever
the peer does. The vocabulary is in `wire.rs`: `Hello`, `Ping`, per-job
messages under a fencing token (job id and epoch), `WantSnapshot`,
`WantChunks`. A caller asks through one typed method per request and a
service answers through one, so a request never gets an answer of the wrong
kind. A chunk is requested by id, never by offset; after a dropped
connection the receiver asks again for what its store does not hold, and a
chunk is trusted only after its bytes hash to its id. A chunk transfer is
cut only when no byte arrives for its idle limit, so a slow link that keeps
moving finishes.

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

## What the tests prove

Two nodes in one process on loopback, on real QUIC and real TLS over TCP:
the hello exchange, a dial to the wrong id failing inside the handshake
without a fallback, an unpaired dialer never surfacing at accept, an
oversized frame getting no answer while the connection survives, a slow
peer tripping the deadline, a chunk transfer resuming after a dropped
connection with only the missing chunks travelling again, three concurrent
3 MiB echo streams under backpressure not crossing, a dropped connection
handle keeping its live streams, a silent socket not holding up a paired
dialer, a black-holed address costing the dial timeout and no more, a
reused hello reporting no false clock skew, and QUIC dialed first with TCP
as the fallback when UDP is a black hole.

In-memory handshakes prove that a certificate without its key fails on
either side and that an unpaired node cannot resume its way back in. The
multiplexer's own tests drive it with a hand-written peer: a dropped stream
never comes back, a stream over the cap is reset, a reset stops the writer,
and streams written out of open order all arrive. Crash tests kill a child
process while it writes the node key or the peer book and find no key or a
whole one, the old book or the new one. The codec has property tests and
random bytes never panic a decoder.

## Not in this phase

Key rotation (a compromised key means re-pair), a pairing code, mutual
pairing over the connection, mDNS address refresh on a LAN, a hub or
relay, and NAT traversal.
