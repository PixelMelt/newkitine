# 25. The share catalog cache is fail-open by design

Status: Accepted
Nicotine+: Matches: Nicotine+ rescans when its share databases fail to load

## Context

The catalog cache (ADR 0034) is derived data: every entry can be rebuilt from the files themselves.

## Decision

A missing, unreadable, corrupt or wrong-version cache warns and yields an empty catalog, so the next walk reads every attribute from disk; a failed save warns and the freshly built index keeps serving. Share data itself stays fail-loud: walk errors abort the scan and surface as ShareScanFailed.

## Consequences

This is a deliberate exception to the no-fallbacks rule. Failing the scan over a cache problem would turn a performance sidecar into a correctness dependency.
