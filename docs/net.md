# The network layer

`crates/fabric-net` implements ADR 0005 as ADR 0013 records it. This page says how to run it on the
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

The daemon re-reads the book before each handshake, so a pairing made with
`fabric node pair` while it runs counts at the next connection. A book that
does not parse pairs no one until it is fixed, and the error names the line.
A line holds at most 8 distinct addresses, and a node appears on one line.

A short pairing code, where one side shows a few words and the other types
them to fetch and confirm the id over the connection, is a follow-up. It
needs a key exchange that this phase does not have.

## Wires

Both transports listen on one port, and the node holds them as one
`Endpoint`. A dial tries QUIC for `QUIC_TIMEOUT` (3 s), then TCP with TLS,
which gets 10 s for the connect and the handshake together. The fallback
reports how QUIC failed and the ledger will record which wire served a
resume, so a slow path is visible. A forged handshake fails the dial at
once. Another node answering at one of the peer's addresses, which a stale
address can do, only moves the dial on to the next address.

Each transport accepts in the background: every handshake runs in its own
task with a 10 s limit, at most 32 at once and at most 4 from one source
address. A TCP socket that sends nothing for 2 s is closed, and QUIC
answers an address it has not seen before with a Retry, so neither a silent
host nor a spoofed source holds up a paired peer.

- QUIC (`quinn` 0.11 over `rustls` 0.23): multiplexed streams, migration
  across address changes, keep-alive every 10 s, idle timeout 30 s.
- TCP with TLS (`tokio-rustls`): the same verifiers over a TCP stream with a
  small multiplexer (`u32 stream id | u8 kind | u32 length | payload`, kinds
  data, fin, reset, credit and keep-alive). Each stream has 1 MiB of credit
  in each direction, as each QUIC stream has its own window, so a stream
  nobody reads stops only its own sender. A dropped stream resets the
  peer's writer, a peer may hold 100 streams open, keep-alives go every
  10 s and 30 s of silence ends the connection, and a connection lives
  while its handle or a stream does, as on QUIC.

ADR 0013 records the trait, the message set and the multiplexer as built.

## Framing and messages

A frame is `u32 length | u8 version | postcard(message)`. A control frame
holds one page of ledger records, which fits a record of the ledger's
largest size, 1 MiB, with its envelope; longer ledgers travel page by page.
A chunk frame holds one chunk as the store keeps it, a zstd frame of at
most 8 MiB decompressed, plus the envelope. A reader checks the declared
length before it allocates, so a peer cannot make a node hold more than one
frame per stream, and every bound in a message, from record sizes to a
manifest's length, is checked as the message is decoded.

Every request runs on its own stream and carries a request number and the
milliseconds the caller still allows. The caller's deadline fires whatever
the peer does. The vocabulary is in `wire.rs`: `Hello`, `Ping`, per-job
messages under a fencing token (job id and epoch), `WantSnapshot`,
`WantChunks`. A caller asks through one typed method per request and a
service answers through one, so a request never gets an answer of the wrong
kind. A per-job request runs to its end on the serving node even when the
caller's deadline passes first; the caller treats that as an unknown
outcome and asks again. A chunk is requested by id, never by offset; after
a dropped connection the receiver asks again for what its store does not
hold, and a chunk is trusted only after it decompresses, within 8 MiB, to
bytes that hash to its id. A chunk transfer is cut only when no byte
arrives for its idle limit, so a slow link that keeps moving finishes.

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
without a fallback, another node at a stale address not ending a dial, an
unpaired dialer never surfacing at accept, a pairing written by another
process after bind counting at the next handshake, an oversized frame
getting no answer while the connection survives, a slow peer tripping the
deadline, a job request past its budget still running to its end, a chunk
transfer resuming after a dropped connection with only the missing chunks
travelling again, three concurrent 3 MiB echo streams under backpressure
not crossing, a stream read later not holding up the one read now, a
dropped connection handle keeping its live streams, a silent socket not
holding up a paired dialer, a black-holed address costing the dial timeout
and no more, and QUIC dialed first with TCP as the fallback when UDP is a
black hole.

In-memory handshakes prove that a certificate without its key fails on
either side and that an unpaired node cannot resume its way back in. The
multiplexer's own tests drive it with a hand-written peer: a dropped stream
never comes back, a stream over the cap is reset, a reset stops the writer,
a peer that sends past its credit ends the connection, a silent peer ends
it after the idle limit while keep-alives keep an idle one up, the writer
flushes the wire when its queue runs empty, and streams written out of open
order all arrive. The chunk stream refuses a chunk that does not hash to
its id or decompresses past 8 MiB, one not asked for or sent twice, and a
done message with chunks missing; a cancelled read ends it. A hello from
another protocol version is refused. One source address holds at most 4
handshakes, and a TCP socket silent for 2 s is closed. A peer book that
does not parse trusts no one, and a line past its bounds is refused with
its number. Crash tests kill a child process while it writes the node key
or the peer book and find no key or a whole one, the old book or the new
one. The codec has property tests, random bytes never panic a decoder, and
the largest page of records fits the control frame.

## Not in this phase

Key rotation (a compromised key means re-pair), a pairing code, mutual
pairing over the connection, mDNS address refresh on a LAN, a hub or
relay, and NAT traversal.
