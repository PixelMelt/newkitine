# 20. Chat history has two owners: the database and the browser

Status: Accepted
Nicotine+: No counterpart

## Context

Chat messages are the one unbounded, append-only stream in the UI.

## Decision

The projection holds no chat messages, private or room. The database is durable history served over REST; the browser merges that history with live events, reconciling on database message identity. Room views in the projection carry only membership.

## Consequences

Snapshots stay bounded (ADR 0022) and history survives reconnects without the projection replaying it.
