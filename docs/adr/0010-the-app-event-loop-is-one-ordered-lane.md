# 10. The app event loop is one ordered lane

Status: Accepted (fatal-queue sentence superseded by libsoulseek ADR 0038)
Nicotine+: No counterpart

## Context

Room lifecycle, chat, session disconnect and user state have cross-domain ordering dependencies: a disconnect atomically clears rooms that queued room events would then race.

## Decision

Client events apply strictly in order on a single task, including their database writes. Transfers get a dedicated worker because their ordering is self-contained behind actor-authoritative snapshots (ADR 0015).

## Consequences

Per-domain worker lanes were considered and rejected. A database slow enough to back up the event queue is the "app cannot keep up" condition that the fatal queue policy (libsoulseek ADR 0007) already covers.

The fatal queue policy was wrong: an ordinary burst of search answers filled the queues and killed the process. Under libsoulseek ADR 0038, a slow event loop now holds the client back, which holds the network back, which stops reading sockets, so the lane stays ordered and nothing is lost or fatal.
