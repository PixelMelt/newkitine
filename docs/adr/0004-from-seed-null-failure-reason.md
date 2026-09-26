# 4. TransferPhase::from_seed maps a NULL failure_reason to an empty reason

Status: Accepted
Nicotine+: No counterpart

## Context

Failed transfer rows may legitimately carry a NULL `failure_reason`: rows that predate migration 2, which extracted reasons from the old status strings.

## Decision

`from_seed` uses `unwrap_or_default` for the reason of a failed row.

## Consequences

Mapping NULL to an empty reason handles real data, not a failed code path. Making it an error would crash boot on valid legacy databases. A backfill migration plus an `expect` would be the stricter form; it has not been worth a schema round for an empty string.
