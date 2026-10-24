# 48. Transfer totals live in memory and count delivered bytes

Status: Accepted (supersedes ADR 0036 in part)
Nicotine+: No counterpart

## Context

The profile and statistics endpoint need the same lifetime totals without querying from the client actor.

## Decision

Transfer totals load at boot from `transfer_history` and update in the transfer worker immediately after the history transaction commits. The profile and statistics endpoint read snapshots. Bytes are delivered bytes, using `COALESCE(bytes, size)` for old rows; commit e75dda2 already changed the statistics SQL to that meaning.

## Consequences

Both readers see one in-memory total, and only committed positive deliveries advance it.
