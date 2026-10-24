# 36. Repeat downloads are capped per file, not convicted per user

Status: Accepted (library half moved to libsoulseek ADR 0022; statistics sentence superseded by ADR 0048)
Nicotine+: No counterpart

## Context

The behaviour sweep convicted a peer as abusive once `transfer_history` held more than three rows for one `(username, virtual_path)` within fourteen days. A row was written for every finished upload segment carrying the full file size, so a peer whose connection stalled and resumed three times had "downloaded the file four times" without receiving it once. Humans who had already messaged to say they were not bots (their verdict cleared to Clean, which is sweep-eligible again) were re-convicted the next morning by the same counter and silently lost every download.

## Decision

`transfer_history` records `bytes` actually delivered per finished segment alongside `size`. Rows written before the column existed have `bytes` NULL, the same "unknown" `speed_bps` already uses, and never count: those inflated segments are the evidence this record retires. Repeats are measured as `SUM(bytes) >= REPEAT_DOWNLOAD_LIMIT * size` per `(username, virtual_path)` inside the window, counted from the peer's `counters_reset_at`. ADR 0048 supersedes the earlier statistics meaning and counts delivered bytes with `COALESCE(bytes, size)`.

Crossing the limit denies further requests for that one file: the app calls `Client::deny_file` with a TTL to the end of the window and its own refusal text (libsoulseek ADR 0022, ADR 0034). No verdict, restriction, or evidence is written; every other file the peer requests is unaffected. The check runs once per delivered upload against the indexed `(username, virtual_path)` and once at boot over the window. There is no sweep for it.

A private message, or the clear endpoint, forgives the peer: verdict cleared if convicted, `counters_reset_at` moved to now, and the client's file denials for them dropped with `Client::clear_file_denials`. The cap applies at every filter level and to buddies, since it caps bandwidth spent on one file rather than judging the peer.

## Consequences

A broken client looping on one file is stopped at that file and told why. No one is blocked from the share for retrying. Downloads still record `bytes = size`, since the download side keeps no resume offset. The Guarded level now means search-flood blocking only.
