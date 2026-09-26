# 1. ConnControl stays one enum

Status: Accepted
Nicotine+: No counterpart

## Context

One control channel follows a connection across its phase transitions (pre-init, then the message loop or the file loop). Audits keep proposing per-phase control types.

## Decision

Keep a single `ConnControl` enum. The `unreachable!` arms in the connection loops are fail-loud guards on actor routing invariants, which is exactly the failure mode we want.

## Consequences

Splitting into per-phase types forces a channel handoff at each transition, which races against controls already queued on the old channel. Two independent audit sessions examined the split and both withdrew it.
