# 44. Client commands are closures run on the actor

Status: Accepted
Nicotine+: No counterpart

## Context

Every operation on the client actor was declared three times: a `ClientCommand` variant, a `Client` method that built it, and a dispatch arm that unpacked it and called one actor method. No variant was built or matched anywhere else, nothing logged or compared commands, and results travelled back through a oneshot sender threaded into each actor method.

## Decision

`Client` (in `client/actor/handle.rs`) sends the actor a boxed `FnOnce(&mut ClientActor)`. `Client::run` queues a closure; `Client::ask` queues one and awaits its return value. Each public method is the one declaration of its operation, and actor methods return their result instead of taking an acknowledgement sender. The methods stay async and awaited (ADR 0009).

## Consequences

Adding an operation touches one place. Commands are no longer data, so they cannot be logged or asserted on; the network actor keeps its `NetworkCommand` enum because its tests assert on commands and its sends are the hot path. Each command costs one small allocation at human rate.
