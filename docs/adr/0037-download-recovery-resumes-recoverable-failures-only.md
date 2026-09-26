# 37. Download recovery resumes recoverable failures only

Status: Accepted
Nicotine+: Diverges: Nicotine+ re-enqueues every failed download when its uploader comes online

## Context

Uploaders drop their upload queue when they disconnect or restart and rely on downloaders to send `QueueUpload` again. Nicotine+ watches every user with a failed download at login, marks pending downloads "User logged off" when the uploader goes offline, and re-enqueues all of that user's failed downloads the next time a status other than offline arrives. It also retries connection failures every 180 s and local I/O failures every 900 s, and holds downloads the uploader refused with a queue-limit reason as queued, re-sending them in small batches once that user's queue drains.

## Decision

The client actor mirrors all of this except the breadth of the online resume. A download failure is classified from its persisted reason into offline, connection (`connection timeout`, `connection closed`, `request timed out`, `upload failed`, `Pending shutdown.`), I/O (`local file error`, `cannot place finished download`, `File read error.`), or terminal (every other peer rejection). Only the first three are watched, rewritten to offline when the uploader goes offline, and resumed when the uploader comes back; connection and I/O failures also retry on their timers from the periodic sweep. Terminal rejections such as `File not shared.` or `Banned` stay failed until the user retries them by hand.

Queue-limit refusals (`Too many files`, `Too many megabytes`, `User limit of …`) move the download to a `Limited` phase that the projection shows as queued. The sweep releases up to `max(5, queued - 1)` of them once the user has no queued download left, the batch size Nicotine+ records at refusal time.

Watches are owned by `Users`: one `WatchUser` plus `GetUserStatus` per user per session regardless of how many files are enqueued, and removing a buddy keeps the watch while a download still needs it.

## Consequences

A peer that refused a file is not asked for it again at every login or every time they come online, which is the traffic Nicotine+ generates for rejected downloads. Failure reasons are part of the recovery contract: renaming one moves it between classes, and rows persisted under an old reason keep the class that string maps to.
