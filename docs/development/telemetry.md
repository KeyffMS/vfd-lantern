# Telemetry pipeline

`TelemetryPipeline` in `lantern-app` is the single application-owned path from completed `PollExecutor` reads to current values, history and downstream telemetry consumers. It does not own Modbus timers and never issues RTU requests; `PollPlan` and `PollExecutor` remain the only cyclic-read source.

## Data and metadata boundary

A successful block is split according to the exact `PollPlan` version that produced it. Each parameter slice is decoded with the codec from the active immutable `ValidatedDeviceProfile`. `TelemetrySampleCore` stores only variable data: session and parameter IDs, raw registers, engineering value, quality, monotonic and UTC timestamps, and request ID. Name, description, quantity, unit, scaling and encoding remain profile metadata and are not copied into samples.

## LatestValues

`LatestValues` is the application SPoT consumed by later TUI work. Every parameter keeps `last_good` separately from `current_quality`, `last_attempt_at`, `last_error`, expected period, maximum age and calculated age.

Errors do not erase `last_good`. A value may satisfy a write guard only when the current quality is `Good` and the last-good age is strictly below `maximum_age`. This check is fail-closed even in the short interval before the freshness worker publishes the explicit `Stale` transition.

A connection owner can call `mark_disconnected()` to atomically move every active parameter in the logical session to `Disconnected` while retaining previous last-good samples for presentation and diagnostics.

## Freshness and time

Freshness uses the same application `MonotonicClock` used by the polling and transport layers. UTC is annotation only and cannot affect deadlines or freshness. The pipeline schedules the next stale transition from the earliest active last-good deadline; it does not create a second Modbus polling timer.

## Bounded history

History exists only for parameters whose active `PollPlan` slice requires history. Enabling the last history subscription allocates an empty bounded `VecDeque`; removing it releases the buffer immediately after the plan swap.

Three limits apply at once:

- maximum samples per channel;
- monotonic retention duration;
- global estimated memory budget.

When limits are exceeded, the oldest data is removed deterministically. Timeout, protocol, decode, stale and disconnect states are stored as explicit gaps, so renderers never interpolate through bad quality.

`render_history(parameter, width)` downsamples directly from the channel deque without first cloning the complete history. The min/max bucket algorithm retains local extrema and time order. Conversion to `f64` is confined to this rendering model; enum/bitfield values and non-finite floats are rendered as gaps.

## Distribution and backpressure

The TUI side uses a `watch` snapshot and therefore coalesces superseded states. CSV, fault and diagnostics consumers use independent bounded channels and `try_send`; none can delay `PollExecutor` or `BusActor`. Overflow is never silent: the pipeline exposes separate drop counters for all bounded event consumers.

Later issues must consume these application outputs instead of creating parallel telemetry state or direct Modbus timers:

- #12/#14 consume `LatestValues` and `render_history`;
- #18 consumes the bounded fault feed and owns fault-event/freeze-frame policy;
- #19 consumes the bounded CSV feed and records its own explicit logging gaps.

## Diagnostics

`TelemetryPipelineStatistics` reports attempts, good sample rate, decode errors, stale/disconnect transitions, quality gaps, history channels/points/bytes, snapshot publications, unknown-plan results and per-consumer drops.

`BusStatisticsSnapshot` remains owned by `BusActor`, and `PollExecutorStatistics` remains owned by the polling executor. The application may compose these immutable values with the active `PollPlan` and pipeline statistics in `DiagnosticsSnapshot`; presentation code must not calculate competing transport percentiles.

## Conformance

The test suite covers multi-parameter block decoding, last-good preservation, monotonic stale/recovery, dynamic history subscriptions, bounded multi-hour history, slow-consumer drops, extrema-preserving downsampling and float special values. `lantern-sim` tests reuse the existing PTY/RTU harness to exercise timeout, Modbus exception, bad frame, physical disconnect and recovery without introducing a second transport simulator.
