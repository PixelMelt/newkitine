# 23. Download placement runs on a blocking worker with a Placing phase

Status: Accepted
Nicotine+: No counterpart

## Context

The client actor must never do unbounded filesystem work.

## Decision

On completion a transfer enters `TransferPhase::Placing` (externally still "transferring"); rename/copy runs via `spawn_blocking` and the result returns through a channel the actor selects on. Copy fallback happens only on a cross-device rename failure; other errors fail the transfer loudly. Placing counts as active (blocks duplicate enqueue) but is excluded from the disconnect reset, so placement survives a server drop.

## Consequences

A slow destination filesystem stalls one transfer, not the actor.
