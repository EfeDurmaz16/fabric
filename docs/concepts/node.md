# Node

A node is one `fabric serve` process and its state directory (`$FABRIC_HOME`, `~/.fabric` by default): the store, one ledger file per job, the managed journal, the peer book and the control socket `fabric.sock` the CLI and the SDKs talk to ([ADR 0016](../adr/0016-node-and-cli-as-built.md)).

Identity. A node is named by its ed25519 public key, printed as `fab1` and 52 base32 characters by `fabric node id`. That key is also the TLS identity pinned on every connection: there is no certificate authority and no central server ([ADR 0005](../adr/0005-transport-and-node-identity.md), [ADR 0013](../adr/0013-network-protocol-as-built.md)).

Pairing. You tell one node the id and address of another with `fabric node add <id> <host:port>`, and the other node the first one's id. Pairing is the only way trust is made; discovery is never trust. The side that dials must be able to reach the other's address. Nothing finds a peer whose address changed; pair again with the new one.

Wires. Nodes talk over QUIC, with TCP plus TLS on the same port when UDP is blocked (`--wire auto`, `quic` or `tcp-tls`).

The node supervises the harnesses it launched: when it learns a higher epoch for a job it runs, it stops that harness. It must be running for every job command.

`fabric doctor` prints the node's facts, and `fabric doctor --peer <id>` compares them with a paired node's before a move: user and home, tool and harness versions, logins as yes or no, clock offset.
