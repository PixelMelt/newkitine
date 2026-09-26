# 22. Bounded projection payloads serialize under the lock; unbounded ones are Arc

Status: Accepted
Nicotine+: No counterpart

## Context

Snapshot and search serialization are bounded by the retention caps (ADR 0018); browse trees are not.

## Decision

Snapshot and search serialization happen under the projection read lock; with the caps they are bounded and a clone to serialize outside would cost the same. `BrowseView` holds Arc'd folder lists: readers clone the Arc under the lock and do all tree-walking and serialization after release. Chat history is never in the projection (ADR 0020).

## Consequences

Lock hold time stays proportional to a bounded payload.
