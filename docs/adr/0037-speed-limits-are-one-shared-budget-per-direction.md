# 37. Speed limits are one shared budget per direction

Status: Accepted
Nicotine+: Matches the default "limit total" behaviour; the mechanism differs (Nicotine+ divides the limit by the active transfer count)

## Context

Every file connection built its own throttle and paced itself against the full configured `upload_bps`/`download_bps`, so N concurrent transfers moved N times the limit. The throttle also slept inside the download loop after the idle deadline had been set, so at low limits a 64 KiB read could sleep past the 60 s idle timeout and the next iteration reported "download stalled".

Nicotine+ recomputes `limit // total_transfers` per direction and caps each connection's per-tick recv/send at that share.

## Decision

`TransferLimits` holds one `Bandwidth` per direction, shared by every file connection through `SharedLimits`. A `Bandwidth` is a single virtual clock: after a connection moves `count` bytes it charges them, advancing the shared `paid_until` by `count / limit` from `max(paid_until, now)`, and waits until the returned instant before its next read. Waits are cancellable by `ConnControl::Close`. Reads are sized to a quarter second of budget, capped at the 64 KiB buffer, so one charge cannot stall a transfer for long and a limit change takes effect within a fraction of a second. A limit of 0 means unlimited and charges nothing.

The download idle timeout is armed per read, after any throttle wait, so intentional waits never count as idleness. Upload reads are also capped at the advertised size remaining.

There is no per-transfer limit mode (Nicotine+'s `limitby` off); the configured limit is always the total.

## Consequences

The aggregate rate per direction matches the configured limit regardless of how many transfers are active, and the budget is spread across connections in charge order rather than by an explicit transfer count, so no count has to be kept in sync with connection lifetimes. The first chunk of a new connection is moved before it is charged, a burst bounded by one quarter-second chunk per connection.
