# 6. App feature files split only at ownership boundaries

Status: Accepted
Nicotine+: No counterpart

## Context

`chat.rs`, `search.rs`, `interests.rs` and `stats.rs` each hold their domain state, SQL, event appliers and HTTP handlers in one file.

## Decision

A feature becomes a folder when, and only when, an ownership boundary appears. `transfers/` and `users/` were split because their persistence and HTTP surfaces grew genuine sub-owners.

## Consequences

Files are not split because they mix concerns at small scale; that split would produce folders of three tiny files with no owner of their own.
