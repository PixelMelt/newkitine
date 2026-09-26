# 33. Walking the shared folders on startup is opt-in

Status: Accepted (rewritten with ADR 0034)
Nicotine+: Diverges: Nicotine+ defaults `rescanonstartup` to on

## Context

A full tree walk of a large share takes minutes.

## Decision

`scan_on_startup` (default false) decides whether boot walks the disk. Boot always installs the cached catalog first (ADR 0034), so shares are served either way; the walk only picks up files changed since the last scan.

## Consequences

An absent index remains a supported state on the very first run: login reports 0/0, searches and browses return empty, and requests are answered "File not shared." until the first walk completes. Saved share edits always scan regardless of this setting.
