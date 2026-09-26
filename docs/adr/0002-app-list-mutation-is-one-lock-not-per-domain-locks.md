# 2. App.list_mutation is one lock, not per-domain locks

Status: Accepted
Nicotine+: No counterpart

## Context

Durable list mutations (buddies, bans, ignores, IP bans, wishlist, interests, chat partners, rooms) are issued by HTTP handlers and by the event lane, and must order the DB → client actor → projection write sequence against a concurrent opposite mutation of the same key.

## Decision

All of them serialize behind a single tokio mutex on `App`.

## Consequences

These are rare, human-driven operations; contention is negligible and one lock is impossible to deadlock against itself. Per-domain locks would add lock-ordering surface while buying nothing measurable.
