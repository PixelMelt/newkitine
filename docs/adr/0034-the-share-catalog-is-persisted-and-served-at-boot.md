# 34. The share catalog is persisted and served at boot

Status: Accepted (supersedes ADR 0007 in part, ADR 0030 and ADR 0031; refines ADR 0033)
Nicotine+: Matches: Nicotine+ persists its share databases, serves them at login, and keeps serving the old list during a rescan

## Context

The index was rebuilt from a disk walk at every boot and dropped on every share-config change. On the reference 850k-file share the walk takes minutes, and during that window we advertised 0/0, answered nothing, and could not resolve uploads. Five decisions existed purely to survive that window: inbound request deferral with an overflow policy (ADR 0030), deferral of our own outbound queue requests so we would not ask peers for files while advertising zero shares, the `awaiting_share_index` gate threaded through the download paths, the opt-in boot scan (ADR 0033), and wholesale upload revocation on share edits (ADR 0007). That is an epicycle cluster built around a missing cache.

## Decision

`shares::walk` produces a `ShareCatalog` (folders with canonical real paths and buddy flags; files with names, sizes, mtimes and audio attributes). After every successful walk the catalog is saved to `share-catalog.gz` next to the config file (a small binary encoding under gzip; ADR 0025 covers failure). Every scan job loads that cache first. When the job has `install_cached` set (boot, and every share-config change) the cached catalog is restricted to the current configuration by `shares::restrict` (a folder survives when its root virtual name is configured and its real path lies under that share's canonical path; the buddy flag comes from the configuration) and installed immediately. When the job walks, the loaded cache is the attribute cache (real path + size + mtime) and the walk's catalog installs when it finishes. The old index keeps serving until a replacement installs. There is no request deferral anywhere: with no index (first run, or an empty cache) requests are answered "File not shared.", exactly like a fresh Nicotine+.

## Consequences

Boot serves shares in seconds instead of minutes; after the first scan we never advertise 0/0 again. The deferral subsystem (`pending_requests`, `PENDING_REQUEST_LIMIT`, `awaiting_share_index`, the `defer_requests` parameters, `pending_queue_requests`, `flush_queued_requests`, the cache save-slot task) is gone. The privacy window after removing a share is the cache-restrict install time (seconds), not the walk time. Changing `share_filters` is applied by the walk only, so the restricted install may briefly serve filtered names. A rename of a share's virtual name drops it from the restricted install and re-reads its attributes on the walk. The legacy `scan-cache.json.gz` is no longer read.
