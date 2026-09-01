# Fault diagnostics and freeze-frame

VFD Lantern treats faults as read-only diagnostics. Fault meaning is resolved only from the active `ValidatedDeviceProfile`; no device-specific fault code, bit name, register address, or reset command is hard-coded in the TUI.

## Source and transitions

A profile declares one optional fault source as `scalar_code` or `bit_set` plus an explicit `no_fault` value. Profile validation checks source encoding and width, scalar code representability, one-hot bit masks, uniqueness, and freeze-frame references.

Only `Good` telemetry observations can create a transition. Scalar sources produce `Raised`, `Changed`, and `Cleared`. Bitset sources produce one atomic, deterministically ordered `BitsChanged { raised, cleared }` event. Unknown scalar values and unknown set bits are retained as `Unknown(raw)` and are never interpreted as no-fault.

Repeated observations of the same state do not create duplicate events; they update `last_observed_at` on the current event.

## Polling and bus priority

The periodic fault source is registered with the common `PollPlanner` as `SubscriptionReason::Fault`, bounded by an explicit maximum age. The planner maps this periodic demand to `TelemetryCritical`; periodic fault traffic cannot request `SafetyOneShot`.

A diagnostic freeze-frame is a bounded one-shot plan built by the same application planner. Its reads use `Interactive`. The plan is limited to 64 parameters and ordinary Modbus read limits. Queue-full, timeout, decode, disconnect, or partial-read failures reduce freeze-frame completeness but do not remove the fault event.

Fault diagnostics never create a write request. There is no fault-reset API or TUI action in 1.0.

## Freeze-frame

Each event keeps two separate sets of observations:

- `pre_fault`: the application-owned `LatestValues` snapshot, including quality and age;
- `captured`: fresh one-shot values, with raw registers, decoded engineering value, quality and timestamp.

Completeness is `Pending`, `Complete`, `Partial`, or `Unavailable`, with explicit errors retained on the event.

## Timeline and export

`FaultTracker` is the only fault timeline for a logical session. It retains at most 256 events and exposes an eviction counter. Acknowledge is local presentation/application state only; it never sends a bus request.

Exports are allowed only while the event identity matches the active Verified session (`SessionId`, device fingerprint, and profile hash). Files use the `.vfdlantern-fault.json` suffix under the XDG data fault-report directory. The storage adapter writes canonical JCS JSON with a SHA-256 digest using an atomic fsync/rename path and mode `0600`.

Simulator scenarios are profile-hash-bound as well: fixtures carry the exact semantic profile hash, so fault-metadata changes fail closed until their expected hash is deliberately refreshed.

The Faults screen provides timeline selection, local acknowledge, export, filters for unacknowledged/unknown events, and navigation to the source parameter. It deliberately exposes no reset command.
