# 11. Owner-internal sibling imports do not route through barrels

Status: Accepted
Nicotine+: No counterpart

## Context

Barrels are the contract for consumers outside an owning module (app importing client, client importing network).

## Decision

Within an owner's subtree (protocol files using wire.rs, transfers using client::users, network actors using network::conn) direct imports are the norm.

## Consequences

Routing internals through the owner's own barrel adds indirection with no boundary being defended.
