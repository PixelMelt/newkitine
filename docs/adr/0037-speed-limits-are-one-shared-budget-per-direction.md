# 37. Speed limits are one shared budget per direction

Status: Accepted
Nicotine+: Matches the default "limit total" behaviour; the mechanism differs (Nicotine+ divides the limit by the active transfer count)

## Context

Every file connection built its own throttle and paced itself against the full configured `upload_bps`/`download_bps`, so N concurrent transfers moved N times the limit. The throttle also slept inside the download loop after the idle deadline had been set, so at low limits a 64 KiB read could sleep past the 60 s idle timeout and the next iteration reported "download stalled".

Nicotine+ recomputes `limit // total_transfers` per direction and caps each connection's per-tick recv/send at that share.

## Decision

`TransferLimits` holds one `Bandwidth` per direction, shared by every file connection through `SharedLimits`. A `Bandwidth` is a single virtual clock, `paid_until`. Before every read, including a connection's first, a transfer waits until `paid_until`, then takes a grant sized to a quarter second of its share of the budget, `limit / active transfers / 4` (at least one byte, capped at the 64 KiB buffer and the bytes remaining). Each transfer holds an `Active` guard on its direction for its lifetime, so the count cannot drift from connection lifetimes. Sizing by share keeps every transfer's gap between sends near a quarter second however many are active, well inside the 60 s inactivity timeouts on both ends. After moving `count` bytes it charges them at the rate the grant was issued at, advancing `paid_until` by `count / limit` from `max(paid_until, now)`. Download grants apply to the socket itself: bytes already buffered by the connection's `BufReader` (read alongside the init) are drained first, then reads go straight to the socket so the buffer cannot pull 8 KiB past the grant. Waits are cancellable by `ConnControl::Close`. A limit change resets `paid_until` to now and wakes every waiter, so a new limit applies at once and debt run up under the old one is forgiven. A limit of 0 means unlimited and charges nothing.

The download idle timeout is armed per read, after any throttle wait, so intentional waits never count as idleness. Upload reads are also capped at the advertised size remaining.

There is no per-transfer limit mode (Nicotine+'s `limitby` off); the configured limit is always the total.

## Consequences

The aggregate rate per direction matches the configured limit regardless of how many transfers are active, and unused share is not stranded: a transfer that moves less than its grant charges only what it moved. Connections woken at the same `paid_until` each move one grant before charging, a burst bounded by one quarter-second chunk per connection that the following waits pay back. New connections and small files wait on the same clock, so connection turnover cannot bypass the limit.
