# Snapshot

A snapshot is an immutable capture of a job's workspace and its harness session at one moment, made by `fabric job suspend`. It is content-addressed: its id is the blake3 hash of its manifest, and every file in it is stored as chunks named by their hash ([ADR 0004](../adr/0004-content-addressed-snapshots.md), [ADR 0014](../adr/0014-store-as-built.md)).

What a snapshot holds:

- the git state, as a bundle with two private commits for the index and the working tree;
- the ignored files, each with its class and, for a cache, the carry key it was made under;
- the harness's state tree with its own manifest, redacted;
- the paths it left out and why: secrets denied, special files, nested repositories.

The node keeps snapshots in its store: a file of 8 MiB or less is one chunk, a larger file is split by FastCDC, every chunk is compressed with zstd, and a SQLite catalog counts the references to each. A move sends only the chunks the other node lacks, so a job that goes back and forth sends little the second time. `gc` removes only chunks no snapshot reaches, after a grace period.

A restore writes a snapshot into an empty directory and renames it into place, or in place into a workspace whose marker says it holds only published bytes. It never writes over work that is not in a snapshot. See [what moves and what does not](../start/what-moves.md).
