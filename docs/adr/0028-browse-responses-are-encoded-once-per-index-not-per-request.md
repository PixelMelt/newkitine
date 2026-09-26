# 28. Browse responses are encoded once per index, not per request

Status: Accepted
Nicotine+: Matches: Nicotine+ builds compressed share lists at scan time and drops repeat requests within 0.4 s

## Context

A browse request is a 4-byte message answered with megabytes.

## Decision

`SharesIndex::from_catalog` encodes the public and buddy share-list frames on the scan worker and keeps them; a browse request is a frame clone (about 3 ms for a 19 MB frame) instead of a fresh catalog walk plus zlib pass (measured 3.7 s per request on 850k files). Banned peers and the no-index state get a static empty frame. Repeat requests from one peer within 400 ms are dropped.

## Consequences

The cached frame removes the CPU amplification; the 400 ms window bounds the bandwidth amplification. The old encode-then-revalidate loop treated "still banned" as a change and re-queued forever.
