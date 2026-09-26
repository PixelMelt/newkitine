# 27. Scan lifecycle vocabulary

Status: Accepted
Nicotine+: No counterpart

## Context

Scans are cancellable, coalesce, and now deliver an index in up to two steps (ADR 0034).

## Decision

`Sharing.running` is the single owner of "a scan job is in flight" (set by `spawn_scan`, cleared by the job's `Done` update). A `ScanJob` is `{ install_cached, walk }`; boot is `{ true, scan_on_startup }`, a share-config change is `RELOAD` `{ true, true }`, a manual or daily rescan is `RESCAN` `{ false, true }`. `start_scan` bumps the generation, and if a job is running it flips that job's cancel flag and merges the request into `pending` (both flags OR together), so rescans coalesce rather than run concurrently. The job sends `ScanUpdate::Index` (install now) zero, one or two times and exactly one `ScanUpdate::Done`; updates carrying a stale generation are discarded, indexes on a blocking worker. `SharesInstalled` reports counts whenever an index installs; `ShareScanFinished` / `ShareScanFailed` end the scanning state.

## Consequences

A superseded walk stops at the next folder and returns `ScanError::Superseded` instead of finishing work that would be discarded. Replaced and discarded indexes are never dropped on the actor.
