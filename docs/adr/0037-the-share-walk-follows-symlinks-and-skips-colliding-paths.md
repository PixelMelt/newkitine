# 37. The share walk follows symlinks and skips colliding paths

Status: Accepted (refines ADR 0025 and ADR 0034)
Nicotine+: Matches: Nicotine+ follows symlinked files and folders and skips a folder or file whose decoded path was already shared

## Context

The walk skipped every symlink with a warning, so a library assembled from links shared nothing behind them. It also aborted the whole scan with `DuplicateVirtualPath` when two entries mapped to the same virtual path (two non-UTF-8 names that decode to the same lossy string, or a real backslash and the backslash sentinel), so one odd directory name left the old index serving forever. Duplicate file names inside one folder were not detected at all and `resolve` silently served whichever came first.

## Decision

Symlinks to files and folders are followed. Each root keeps a set of canonical folder paths it has walked; a folder whose canonical path was already walked under that root (a loop, or a second link to the same folder) is skipped with a warning. A folder reached through a link keeps the link path as its real path, so it stays lexically under the share root and `shares::restrict` keeps it. A symlink whose target cannot be resolved (dangling, permission) is skipped with a warning.

A folder whose virtual path was already claimed in this walk is skipped with its subtree, and a file whose virtual name was already claimed in its folder is skipped, each with a warning. The first one walked wins.

## Consequences

Other walk errors (unreadable root or folder, failed stat of a real entry) still abort the scan as ADR 0025 says. Two links to the same folder under one root are shared once, under whichever path is walked first. Symlinks can point outside the configured share root; that is the user's choice, as in Nicotine+.
