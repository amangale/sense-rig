# sense-rig

A miniature sensor-data ingestion pipeline in Rust, built to measure
delivery semantics under injected faults — every lossy behavior is
observable and counted, never silent.

Companion project to
[bidi-rig](https://github.com/amangale/bidi-rig) (same measurement
discipline, applied to gRPC bidirectional streaming, in Go).

## What it measures

A source simulator emits timestamped sensor readings with configurable
duplication and drop probability. An ingestion pipeline deduplicates
on sequence numbers within a bounded reorder window, flags out-of-order
arrivals, and enforces bounded buffers with explicit backpressure.
Every branch has a counter: delivered, duplicate, dropped, late.

## Status

- [x] Phase 1 — domain types, dedup + ordering gate, unit tests
- [ ] Phase 2 — source simulator with fault-injection dial
- [ ] Phase 3 — tokio wiring, mpsc channels, backpressure, live counters

## Run

    cargo test

## Why

Built as a hands-on Rust learning vehicle against a real systems
problem: ingesting robotics sensor data with accountable delivery
semantics. See the bidirectional gRPC predecessor for the streaming
variant of the same experiment.