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
