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
No CA, no name, no date. TLS 1.3 only, ed25519 only, 0-RTT off.

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
(3 s), then TCP with TLS. The fallback is reported with its reason and the
ledger will record which wire served a resume, so a slow path is visible.

- QUIC (`quinn` 0.11 over `rustls` 0.23): multiplexed streams, migration
  across address changes, keep-alive every 10 s, idle timeout 30 s.
- TCP with TLS (`tokio-rustls`): the same verifiers over a TCP stream with a
  small multiplexer (`u32 stream id | u8 kind | u32 length | payload`). One
  stream that nobody reads holds up the others once its 1 MiB buffer is
  full; the protocol reads every stream it opens.

## Framing and messages

A frame is `u32 length | u8 version | postcard(message)`. A control frame is
at most 1 MiB; a chunk frame at most 8 MiB plus the envelope. A reader
checks the declared length before it allocates, so a peer cannot make a node
hold more than one frame per stream.

Every request runs on its own stream and carries a request number and the
milliseconds the caller still allows. The caller's deadline fires whatever
the peer does. The vocabulary is in `wire.rs`: `Hello`, `Ping`, per-job
messages under a fencing token (job id and epoch), `WantSnapshot`,
`WantChunks`. A chunk is requested by id, never by offset; after a dropped
connection the receiver asks again for what its store does not hold, and a
chunk is trusted only after its bytes hash to its id.

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
- On a LAN: pair with the LAN address. With the `mdns` feature a node
  advertises `_fabric._udp` with its id in the TXT record, and a browser
  reports nodes it sees. A seen node that is paired gets its address
  refreshed; one that is not is listed as unpaired and never dialed.
- Two nodes behind symmetric NAT with no tailnet cannot reach each other.
  v0 says so instead of pretending; iroh is the first thing to try if the
  gate fails for network reasons (ADR 0005, alternative 1).

## What the tests prove

Two nodes in one process on loopback, on real QUIC and real TLS over TCP:
the hello exchange, a dial to the wrong id failing inside the handshake, an
unpaired dialer never surfacing at accept, a paired certificate without its
key failing, an oversized frame getting no answer while the connection
survives, a slow peer tripping the deadline, a chunk transfer resuming after
a dropped connection with only the missing chunks travelling again, three
concurrent 3 MiB echo streams under backpressure not crossing, and QUIC
dialed first with TCP as the fallback when UDP is a black hole. The codec
has property tests and random bytes never panic a decoder.

## Not in this phase

Key rotation (a compromised key means re-pair), a pairing code, mutual
pairing over the connection, a hub or relay, and NAT traversal.
