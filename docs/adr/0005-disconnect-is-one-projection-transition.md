# 5. Disconnect is one atomic projection transition owned by session

Status: Accepted
Nicotine+: No counterpart

## Context

A server disconnect must clear rooms, reset buddy statuses, zero the peer count, clear available rooms and then publish the status, without a subscriber observing a half-applied state.

## Decision

`session::disconnected` holds one projection write lock and delegates to `chat::server_disconnected` and `users::server_disconnected`, which take the writer. Rooms are cleared with RoomLeft events, buddy statuses reset to unknown, the peer count zeroed, available rooms cleared, then the Status event.

## Consequences

Moving this into a separate event coordinator would add a layer that owns nothing else.
