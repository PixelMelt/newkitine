# 4. TransferPhase::from_seed maps a NULL failure_reason to an empty reason

Status: Accepted
Nicotine+: No counterpart

## Context

Failed transfer rows may legitimately carry a NULL `failure_reason`: rows written before the column existed, and failed rows whose reason was never recorded.

## Decision

`from_seed` uses `unwrap_or_default` for the reason of a failed row.

## Consequences

Mapping NULL to an empty reason handles real data, not a failed code path. Making it an error would crash boot on a valid database. A `NOT NULL` column plus an `expect` would be the stricter form; it has not been worth a schema change for an empty string.
