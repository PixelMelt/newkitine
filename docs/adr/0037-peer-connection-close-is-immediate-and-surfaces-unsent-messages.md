# 37. Peer connection close is immediate and surfaces unsent messages

Status: Accepted
Nicotine+: Matches: Nicotine+ clears a closing connection's buffers, re-routes later sends to a new connection, and reports unprocessed messages through `peer-connection-closed`

## Context

A peer message connection wrote each frame with `write_all().await` inside its `select!`, so a peer that stopped reading pinned the task, the socket and the frame buffer: neither the idle deadline nor `Close` could run. The idle deadline was only refreshed by complete frames, so a large browse still streaming after 60 s was killed. The actor closed every connection that delivered a search result, even while another response was arriving on it, and sends pushed to a connection that had been told to close, or whose task had already exited, were counted as delivered and silently lost.

## Decision

Writes run on their own task behind a bounded queue; each write must make progress within the 60 s peer idle timeout. Inactivity is measured at the socket: every byte read and every byte written refreshes the connection's last-activity time, which `Traffic` shares between the connection task and the actor.

`Close` is immediate. The reader and writer tasks are aborted and any bytes already handed to the writer are dropped, as Nicotine+ clears `out_buffer`. The actor detaches a connection from its peer-init routing the moment it asks it to close, so later sends open a new connection. Peer messages the connection never took off its control queue, and messages the actor could not push because the task had already gone, are reported as `PeerConnectionError` with those messages as `unsent`.

After a search result, the actor closes the connection only when it is quiescent: every byte received has been delivered as a frame and every send the actor pushed has been written. Connections to our own username are never closed this way.

## Consequences

A stalled peer costs at most 60 s before its connection is torn down, and `Close` always releases the socket at once. A queued browse response that the peer never drains is lost with the connection, the same as in Nicotine+. Unsent requests (queue uploads, folder requests, user info, browse) fail through the same client path as an unreachable peer instead of hanging until their own timeouts.
