# The network layer

`crates/fabric-net` implements ADR 0005 as ADR 0013 records it. This page says
how to run it on the networks Efe has and what it refuses. ADR 0013 holds the
mechanics: the message set, the bounds, the multiplexer and the timers.

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

The daemon re-reads the book before each handshake, so a pairing made with
`fabric node pair` while it runs counts at the next connection. An unpair
also ends a connection that is already open, within 5 s. A book that does
not parse pairs no one until it is fixed, and the error names the line. A
line holds at most 8 distinct addresses, and a node appears on one line.

A short pairing code, where one side shows a few words and the other types
them to fetch and confirm the id over the connection, is a follow-up. It
needs a key exchange that this phase does not have.

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
