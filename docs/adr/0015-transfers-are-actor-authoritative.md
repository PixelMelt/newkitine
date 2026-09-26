# 15. Transfers are actor-authoritative; MySQL is a projection of them

Status: Accepted
Nicotine+: No counterpart

## Context

Transfer transitions originate on the network side (peer messages), where the actor must decide immediately. Durable-list ordering (ADR 0003) would route every transition through the app before acting.

## Decision

The client actor owns transfer state and emits authoritative `TransferSnapshot` transitions; the app persists them through one ordered worker, and a DB write failure is fatal (exit). The actor seeds from the DB at boot.

## Consequences

A crash between acknowledgement and persistence loses at most the in-window transitions, which is accepted and bounded. App-owned prepare/persist/commit was rejected across four audit rounds: it adds a round-trip on the hot path and a second owner for retry and queue logic.
