# 15. MySQL is a projection of the client actor's transfers

Status: Accepted (library half moved to libsoulseek ADR 0010)
Nicotine+: No counterpart

## Context

The client actor owns transfer state and emits authoritative snapshots on its `TransferWork` stream (libsoulseek ADR 0010). Durable-list ordering (ADR 0003) does not apply to them.

## Decision

The app persists the stream through one ordered worker, and a DB write failure is fatal (exit). Boot settles the stored transfers with `TransferSnapshot::settle_after_restart`, writes back the ones it changed, and seeds the actor from them through `ClientBootstrap.transfers`.

## Consequences

A crash between acknowledgement and persistence loses at most the in-window transitions, which is accepted and bounded. App-owned prepare/persist/commit was rejected across four audit rounds: it adds a round-trip on the hot path and a second owner for retry and queue logic.
