# 11. Owner-internal sibling imports do not route through barrels

Status: Accepted (library half moved to libsoulseek ADR 0006)
Nicotine+: No counterpart

## Context

Barrels are the contract for consumers outside an owning module: one feature module importing another, and newkitine importing the `soulseek` crate. The library keeps the same rule for its own modules (libsoulseek ADR 0006).

## Decision

Within an owner's subtree (`users/http.rs` using `users/db.rs`, `behavior/db.rs` using `behavior/policy.rs`) direct imports are the norm.

## Consequences

Routing internals through the owner's own barrel adds indirection with no boundary being defended.
