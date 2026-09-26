# 3. Mutation order is DB first, then client actor, then projection

Status: Accepted
Nicotine+: No counterpart

## Context

Durable state has three holders: the database, the client actor and the projection. A crash can land between any two writes.

## Decision

Write the database first, then command the client actor, then update the projection.

## Consequences

The database is the durable source of truth; a crash after the DB write converges on restart because boot reloads from the DB. Reversing the order could acknowledge state that was never persisted. Transfers are the deliberate exception (ADR 0015).
