# ADR 0003: Defer SQLite Persistence Until After Phase 1

- **Status:** Accepted for Phase 1 planning
- **Date:** 2026-10-21

## Context

Reliable transfer lifecycle and partial-file semantics are not yet implemented. Persisting intermediate state before defining crash recovery could overstate guarantees.

## Decision

Phase 1 keeps jobs in memory. SQLite is introduced in a later phase after durable metadata, migrations, crash recovery, and partial-file semantics have explicit contracts.

## Consequences

- Active jobs are not restored after restart in Phase 1.
- Later database location is per-user application data; WAL mode will be evaluated and its sidecar-file lifecycle tested rather than assumed.
- No database dependency is added during Phase 0.

## References

- SQLite WAL: https://www.sqlite.org/wal.html
