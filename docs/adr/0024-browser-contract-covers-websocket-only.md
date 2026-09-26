# 24. The typed browser contract covers the WebSocket stream, not REST responses

Status: Accepted
Nicotine+: No counterpart

## Context

WebSocket events mutate long-lived replicated stores, where a silently drifted shape corrupts state that outlives the message. REST responses are consumed once at the call site that requested them.

## Decision

The event stream is typed, validated at the socket boundary and pinned by the node-backed contract test. REST payloads are not given a parallel DTO layer.

## Consequences

A REST shape mismatch fails visibly in one component on the next interaction. The contract module's bidirectional imports with feature modules are inherent to an app-owned internally-tagged event enum, not a layering defect.
