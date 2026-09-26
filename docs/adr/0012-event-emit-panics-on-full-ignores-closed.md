# 12. Event emit() panics on Full, ignores Closed

Status: Accepted
Nicotine+: No counterpart

## Context

Consumer death is detected by supervision: `events::run` exiting kills the process. A closed channel during shutdown is not an error.

## Decision

`emit` ignores Closed and panics on Full.

## Consequences

A full event queue means the app cannot keep up with the client actor; losing events would silently diverge state, so the panic is the loud option.
