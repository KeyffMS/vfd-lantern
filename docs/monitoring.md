# Monitoring runtime, Dashboard and Scope

This document describes the read-only monitoring implementation owned by roadmap issue #14.

## Safety and ownership boundaries

Monitoring is available only inside a logical session created by successful, unambiguous Verified identification. The TUI never opens a serial port, creates Modbus requests or owns telemetry state.

The ownership chain is:

1. `ApplicationState` owns the active profile, logical session, Dashboard selection and active Scope channel selection.
2. The composition root owns the one existing `BusActorHandle` and the monitoring tasks.
3. `PollPlanner` is the only producer of periodic read plans.
4. `PollExecutor` is the only monitoring scheduler that sends those periodic reads to the existing `BusActor`.
5. `TelemetryPipeline` owns `LatestValues`, current quality, last-good values and bounded histories.
6. `ApplicationView::monitoring()` is an immutable presentation projection.
7. `UiState` owns presentation-only Scope controls such as pause, pan, zoom, cursor and manual Y ranges.

Dashboard and Scope therefore cannot bypass the planner or create an independent Modbus read path.

## Verified gate and reconnect

Opening a serial adapter does not enable semantic monitoring. The composition root records the `BusActorHandle`, but the monitoring read gate remains closed until `ApplicationState` emits `MonitoringEffect::Start` after successful Verified identification.

On transient transport loss:

- the current telemetry snapshot transitions to `Disconnected`;
- the serial handle becomes unavailable;
- the monitoring runtime and bounded history remain associated with the same logical `SessionId`;
- the read gate remains closed while reconnect identification is running;
- no semantic monitoring request is forwarded to the bus before successful re-verification;
- a successful same-identity reconnect emits `MonitoringEffect::Resume` and reopens the gate.

An explicit user disconnect, shutdown or reconnect identity change stops the monitoring planner/runtime. Old runtime snapshots are rejected by `ApplicationState` when their `SessionId` does not match the active logical session.

## Poll planning and channel deduplication

Dashboard values come from the active validated profile's telemetry preset. Scope channels are session-local and limited to eight active parameters in at most four panels.

Dashboard subscriptions request normal-frequency latest values without history. Active Scope channels request fast-frequency values with history. Both subscription sets are compiled together by the same `PollPlanner`. If Dashboard, Scope and CSV request the same `ParameterId`, the planner retains the distinct subscribers/freshness requirements but emits one physical read demand at the strictest required cadence.

A monitoring plan with rejected subscriptions is not silently accepted. The runtime reports the rejection through the application monitoring error projection.

Removing a Scope channel rebuilds the plan. `TelemetryPipeline::update_plan` then releases history for parameters that no longer require history.

## Quantity and unit axes

A Scope axis is identified exactly by `(QuantityKind, UnitId)`.

Channels may share a panel only when both quantity and unit are identical. This means, for example:

- `Frequency/hz` can share a panel with another `Frequency/hz` channel;
- `Frequency/hz` never shares a panel with `RotationalSpeed/rpm`;
- the same quantity with a different unit uses a separate panel;
- custom quantities/units share only when their stable IDs are identical.

Display labels never participate in axis identity. Two parameters with the same name but different quantity/unit remain incompatible.

There is no automatic unit conversion in 1.0.

## Dashboard projection

Each Dashboard value shows only information supplied by the validated profile and `LatestValues`:

- parameter name and code;
- last-good engineering value;
- unit;
- current quality;
- age of the last-good value;
- age of the last attempt.

A bad current quality does not erase or replace the last-good value. The UI can therefore show, for example, the last known engineering value together with `Timeout`, `Stale` or `Disconnected`.

The Dashboard diagnostics projection includes:

- bus round-trip p95;
- planned and measured RTU utilization;
- telemetry timeout events;
- bus queue-full count;
- skipped poll deadlines and dropped poll results;
- CSV/fault/diagnostics consumer drop counts.

No producer-specific parameter name, register address or fallback drive-state/fault address is hard-coded. If the active profile does not define a semantic item, the Dashboard does not guess one.

## Scope search and channel controls

The Scope catalog is derived only from the active validated profile. Search matches normalized:

- parameter ID;
- code;
- name;
- aliases;
- `QuantityKind`;
- `UnitId`.

Register addresses are deliberately not part of the search surface. Full, partial and zero alias sets are covered by acceptance tests.

`Enter` toggles the selected catalog parameter through `MonitoringAction`, which causes an application-owned subscription change. Moving a channel between panels is also validated in the application layer so incompatible quantity/unit axes cannot be overlaid.

`Clear Scope history` clears only the currently selected Scope histories. It does not clear `LatestValues`, stop polling, or affect later CSV/fault consumers.

## Presentation-only pause, pan, zoom and cursor

Scope presentation controls remain in `UiState` and do not create application effects:

- pause stores the current monotonic render anchor;
- collection, telemetry freshness and downstream consumers continue while paused;
- pan shifts the visible monotonic time anchor;
- zoom changes only the visible time span;
- supported base windows are 10 s, 30 s, 1 min, 5 min and maximum retained history;
- the cursor selects an actual rendered sample or explicit quality gap, never an interpolated value;
- manual Y ranges are stored only in `UiState`.

The application continues publishing bounded immutable snapshots independently of these presentation controls.

## History and rendering

History uses monotonic timestamps. The telemetry pipeline converts engineering values to `f64` only for bounded render history and preserves bad-quality periods as explicit gaps.

The Scope render model:

- ignores gaps, NaN and infinity when calculating autoscale;
- adds padding for a constant finite signal;
- performs bounded chronological bucket compression while retaining finite minima/maxima and explicit gap markers;
- renders gaps as breaks/markers rather than connecting values across missing or bad-quality data;
- caps each published render history to 512 points per active Scope channel.

Monitoring snapshots are emitted no faster than the configured render rate. Settings validation caps that rate at 10 FPS.

The permanent self-hosted performance gate renders a 120×40 `TestBackend` with eight active channels, four panels and 512 retained points per channel. It uses 40 warm-up frames and 400 measured release-mode frames. The enforced #25 budget is p95 <20 ms and p99 <33 ms. CI run #751 measured p95 841 µs and p99 877 µs; the unchanged benchmark gate passed again on final acceptance run #752.

## Runtime consumers

Until their owning roadmap issues are implemented, the telemetry pipeline's CSV, fault and diagnostics event receivers are actively drained by the composition root. This prevents artificial queue drops without introducing placeholder persistence or fault semantics. Their real consumers remain owned by their later roadmap issues.

## Verification

The #14 tests cover the following contracts incrementally:

- Hz and rpm are different semantic axes;
- incompatible axes cannot share one Scope panel;
- Scope accepts exactly eight channels across four panels and rejects a ninth channel;
- equal display labels cannot merge different quantity/unit axes;
- full, partial and zero alias sets preserve semantic catalog/search behavior;
- Dashboard/Scope subscriber deduplication occurs in `PollPlanner` while preserving freshness/history requirements;
- Dashboard + Scope + CSV for the same parameter compile to one physical poll demand with all three subscribers preserved;
- last-good values survive bad current quality in the immutable projection;
- catalog search uses semantic profile metadata and not Modbus addresses;
- pause freezes only the render anchor;
- autoscale ignores gaps/NaN/infinity and gives a constant finite signal a bounded range;
- bounded compression preserves an impulse and an explicit quality gap;
- monitoring starts only after a Verified match;
- Scope changes are application effects that rebuild the shared plan;
- explicit disconnect stops the planner before closing transport;
- reconnect identity mismatch is covered both at the session reducer boundary and through the production PTY RTU stack, and stops the planner before closing the replacement transport;
- process E2E requires zero Modbus traffic before explicit Connect; a successful Match then requires exactly one identification probe plus the first normal-cadence monitoring read, while fail-closed identification cases retain their exact probe-only request counts and every observed request remains a read function;
- the 120×40 / eight-channel / four-panel release benchmark is a permanent CI gate against the #25 p95/p99 render budget.

Final #14 acceptance was verified by self-hosted CI #752 on `vfd-lantern-podman-01`: build, rustfmt, Clippy `-D warnings`, full tests, process E2E, telemetry benchmark, Scope render benchmark, rustdoc, architecture checks and full supply-chain checks all passed.
