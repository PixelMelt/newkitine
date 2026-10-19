# 46. Settings build the local storage and replace it on change

Status: Accepted (refines ADR 0032)
Nicotine+: No counterpart

## Context

libsoulseek ADR 0030 moved shares, share filters and the download directories out of `RuntimeConfig` into `LocalShares` and `LocalDownloads`, which a consumer constructs and hands to the client, replacing them through `set_shares` and `set_downloads`.

## Decision

`Settings::local_shares` builds `LocalShares` from the shares, the filters and `share-catalog.gz` next to the config file, and `Settings::local_downloads` builds `LocalDownloads` from the download directory, the incomplete directory and the username-subfolder flag. `PUT /api/settings` builds both before it stores anything, so an invalid share name, filter or download directory is a 400, and boot exits on one as it does for `runtime_config`. After `apply_config`, the settings update calls `set_shares` only when the shares or filters changed and `set_downloads` only when one of the download fields changed. The `transfers.folder_path` column keeps its name and stores `TransferSnapshot::destination`, which for `LocalDownloads` is the folder path it held before, so stored rows seed unchanged.

## Consequences

A share edit reloads and rescans as before. A download keeps the store of its enqueue, so changing the incomplete directory moves only downloads enqueued afterwards and a queued download resumes from its partial in the old directory until the next restart; downloads restored at boot take the store built from the current settings, so they start over in the new directory and the old partial stays behind.
