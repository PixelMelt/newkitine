# 31. The attribute cache is skipped when the scan changed nothing

Status: Superseded by ADR 0034
Nicotine+: No counterpart

## Context

Saving the attribute cache cost 14 s unbuffered and 0.8 s buffered on 623k entries.

## Decision

`scan` returned no cache when every audio file hit the cache and nothing was pruned, and a save-slot task wrote it after the index installed.

## Consequences

The catalog is now saved after every successful walk, on the scan worker, before the index is handed over (about one second buffered through gzip). That removed the save slot, its Notify and its task; a daily rescan paying one second is not worth a second code path.
