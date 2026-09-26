# 9. NetworkHandle::send is sync try_send + panic; Client methods are async awaited

Status: Accepted
Nicotine+: No counterpart

## Context

The client actor and the network actor form a command cycle; awaiting inside that cycle deadlocks. The app → client boundary has no cycle.

## Decision

Actor-to-actor sends are `try_send` with a panic on Full. `Client` methods await with real backpressure.

## Consequences

Queue overflow between actors is a fatal pacing bug, not a recoverable condition. The app gets backpressure where backpressure is safe.
