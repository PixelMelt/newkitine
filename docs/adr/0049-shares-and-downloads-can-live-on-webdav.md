# 49. Shares and downloads can live on WebDAV

Status: Accepted (refines ADR 0046)
Nicotine+: Diverges: Nicotine+ shares and downloads local files only and always reads audio attributes; local storage with fast scan off keeps its behaviour

## Context

ADR 0046 builds `LocalShares` and `LocalDownloads` only. The library takes any `ShareSource` and `DownloadStore` (libsoulseek ADR 0030) and publishes the share rules and download naming (libsoulseek ADR 0036), so `storage::webdav` implements both against one WebDAV server.

## Decision

`share_storage` and `download_storage` (`local` or `webdav`, default `local`) are chosen independently. The share paths, `download_dir` and `incomplete_dir` are strings whose meaning follows their side's storage: a local path, or a remote path starting with `/` under the WebDAV URL. The incomplete directory defaults to `<download_dir>/incomplete` on either storage, and `NEWKITINE_DOWNLOAD_DIR` sets `download_dir` for whichever storage is chosen. Stored rows without the new fields load as local with fast scan off.

`SettingsState::share_source` and `SettingsState::download_store` replace `Settings::local_shares` and `Settings::local_downloads` and return trait objects; boot and `PUT /api/settings` use them as ADR 0046 describes. `share_storage` and `fast_scan` count as share changes and `download_storage` as a download change, so only the changed side is replaced.

The server is a `[webdav]` section (`url`, `username`, `password`) in `newkitine.toml`. `NEWKITINE_WEBDAV_URL` creates or overrides it, `NEWKITINE_WEBDAV_USERNAME` and `NEWKITINE_WEBDAV_PASSWORD` override its fields, and either of those without a section panics at startup. Boot builds one `WebDav` client for both sides and exits on an invalid URL. Credentials never reach the settings table or the browser: the settings payload carries only `webdav`, whether a server is configured, and the Settings page disables the WebDAV option without one. Choosing WebDAV without a configured server is an error naming the setting and how to configure it.

`WebDavDownloads` streams a download through a fixed 64 KiB pipe into one PUT at its incomplete path and MOVEs it to its destination when complete. Nothing is kept on local disk and nothing resumes: `open` always answers offset 0, so every retry, recovery and restart starts the file over.

`fast_scan` (default off) builds `LocalShares` with `ScanMode::Fast` (libsoulseek ADR 0035). `WebDavShares` always lists without reading files, since reading attributes would fetch every audio file over HTTP, so WebDAV shares require `fast_scan` and refuse to build without it rather than silently ignoring it. Peers see no bitrate or duration for them. Their folder keys carry a `webdav:` prefix: sources that replace one another share one key namespace (libsoulseek ADR 0030), so without it switching share storage would let an upload resolved against a local file continue from a different remote file at the same path; with it, such uploads are denied as any upload whose key leaves the index. WebDAV shares keep no catalog cache, and ADR 0033 has boot only load unless `scan_on_startup` is on, so `WebDavShares::load` lists the whole tree and `scan` lists it again.

Changing `download_storage` while any download in the transfer list is not Finished is a 400, "Finish or clear the N unfinished downloads before switching download storage": a queued, failed or aborted download keeps its stored destination across restarts (ADR 0046) and would be seeded into the other store. The library exposes no transfer snapshot query, so `PUT /api/settings` counts from the projection (ADR 0015) under the settings mutation lock, and `Settings::check_download_switch` decides.

## Consequences

Replacing a WebDAV share source lists the tree twice, since `set_shares` loads and then rescans; so does a boot with `scan_on_startup` on. A server that is down at boot leaves the shares empty until the next rescan or share edit, because a failed load is only logged (libsoulseek ADR 0030).

The guard leaves a window. The projection trails the actor, and downloads are also admitted without HTTP: a folder request's answer is enqueued on the actor. A download the actor admits between the projection read and `set_downloads`, whether a concurrent enqueue or retry, a folder answer, or an admission still queued for the transfer worker, keeps the old store for this session (libsoulseek ADR 0030) and, if still unfinished at the next restart, is seeded into the new store with the old store's destination. An HTTP-side lock would close only the first case, so none was added; clearing and enqueuing such a download again fixes it. A finished download from before the switch also keeps its old destination, which only an API retry after a restart would reuse; the UI retries only failed and aborted downloads.

A zero-byte download to goatdin fails with goatdin's refusal of empty uploads; there is no special case.

The library bounds each download open, write and final shutdown by the 60 s peer idle timeout (libsoulseek ADR 0031), and PROPFIND, MKCOL, MOVE and DELETE carry a 60 s request timeout. A write or a final commit the server takes longer than 60 s to accept fails the transfer, and goatdin may still commit the file at the incomplete path; the next attempt overwrites it and clearing the download deletes it. On a generic server a truncated upload may leave a partial at the incomplete path until it is discarded or the next attempt replaces it.

`MOVE` sends an absolute `Destination` built from the configured URL, so the URL must carry the host the server sees; a proxy that rewrites `Host` breaks placement.

A stored WebDAV choice whose `[webdav]` section is later removed exits at boot, and as in ADR 0032 the UI that would fix it never starts; restore the section or edit the settings row.
