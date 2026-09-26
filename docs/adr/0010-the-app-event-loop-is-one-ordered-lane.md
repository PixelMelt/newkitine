# 10. The app event loop is one ordered lane

Status: Accepted
Nicotine+: No counterpart

## Context

Room lifecycle, chat, session disconnect and user state have cross-domain ordering dependencies: a disconnect atomically clears rooms that queued room events would then race.

## Decision

Client events apply strictly in order on a single task, including their database writes. Transfers get a dedicated worker because their ordering is self-contained behind actor-authoritative snapshots (ADR 0015).

## Consequences

Per-domain worker lanes were considered and rejected. A database slow enough to back up the event queue is the "app cannot keep up" condition that the fatal queue policy (ADR 0012) already covers.
