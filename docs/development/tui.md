# TUI architecture

`lantern-tui` is a presentation adapter. It owns terminal presentation state and Ratatui rendering, but it does not own application/domain state, transport adapters, storage adapters, a session reducer or an effect runner.

## One-way state flow

1. Crossterm `EventStream` produces a terminal event.
2. The keymap maps it to either a presentation-only `UiAction` or an application-owned `ApplicationAction`.
3. `UiAction` mutates only `UiState` (screen, focus, scroll, selection, form/search text, modal and viewport).
4. `ApplicationAction` is dispatched through `ApplicationRuntime`; effects remain application-owned and are executed at the composition root.
5. Asynchronous transport/discovery results return to the same reducer as typed application actions.
6. Rendering reads only immutable `ApplicationView` and `UiState` snapshots.

The TUI never opens serial devices, reads files, runs Modbus requests or creates a second telemetry/session state.

## Verified-only connection wizard (#13)

The Connection screen is the single product path from a disconnected process to a Verified read-only session:

1. **Port** — render the passive udev snapshot, including stable `/dev/serial/by-id` when available, kernel node, manufacturer, VID/PID, serial, driver, product and presence. `r` requests another passive snapshot. `m` edits a Manual path. A `--device` value is only a prefill; it never opens the device automatically.
2. **Profile** — select one already validated `ValidatedDeviceProfile`. `/` searches case-insensitively by profile ID, vendor, family or model. The view shows origin, schema v1 revision, `profile_hash`, `source_hash` and available hardware-verification metadata.
3. **Link** — edit only values admitted by the selected profile: baud, parity, data bits, stop bits and validated Modbus slave ID. The profile owns the defaults, allowed sets, response timeout and RS-485 mode.
4. **Summary** — show the exact adapter/profile/link selection plus the exact bounded read-only identification probe plan (probe ID/description, Modbus table/address/count and expected raw values). This step is still read-only and the serial port is not open.
5. **Explicit Connect** — only an explicit Enter on Summary may emit `ConnectionEffect::OpenPort`. Selection, search, editing and summary navigation emit no serial-open or Modbus effect.
6. **Identification** — after the verified serial open, `lantern-app` performs only the bounded read probes declared by the selected profile, through the production `BusActor`. No blind slave scan, baud scan, profile guessing, write or telemetry polling occurs in this phase.
7. **Result** — only a unique complete `Match` may create the new logical `SessionId` and Verified session. `Partial`, `Mismatch`, `Ambiguous`, transport/protocol `Error`, cancellation or adapter removal never create a Verified session and close the opened transport. There is no `continue anyway` path.

A successful wizard finishes either `PROCESS DISABLED` (normal default) or `DISARMED` when the process was started with `--enable-writes`. The wizard itself cannot arm writes and cannot execute a write.

### Manual paths and discovery failures

Manual selection intentionally fabricates no stable ID, manufacturer, product, VID/PID, serial or driver metadata. The open path still canonicalizes the character device and validates the actual descriptor before constructing the bus actor.

The udev snapshot and hotplug monitor are passive conveniences, not a prerequisite for Manual operation. If monitor subscription is unavailable (for example in a restricted container), the Manual path remains usable. Discovery errors are presented as connection state; they do not trigger an automatic open or scan.

For a successfully opened Manual selection, the composition root also watches only the selected device-path presence. This is not Modbus polling and it does not create a second telemetry scheduler. If the selected path disappears, the watcher emits the existing application/session `TransportLost(PortRemoved)` input; `SessionStateMachine` remains the sole owner of reconnect backoff, reopen, re-identification and identity acceptance. The watcher is generation-bound and terminates when the transport generation changes. Detected adapters continue to use the udev hotplug path instead of this Manual-path fallback.

### Identification evidence and fingerprint

The application retains two views of identification evidence:

- the small domain `IdentificationReport` consumed by `SessionStateMachine` for the safety decision;
- application-owned diagnostics for the UI/offline report: probe ID and description, register block, expected/raw words, quality, elapsed time, match result, errors, overall outcome, profile hash and fingerprint candidate.

Profile-v1 identification probes declare raw words but no engineering codec/scale, so their engineering display is explicitly `N/A (raw-only probe)` rather than an invented value.

The production fingerprint is derived from the selected profile hash, observed probe evidence and the verified adapter identity. When a stable `/dev/serial/by-id` exists, the fingerprint uses that stable identity rather than the renumberable `/dev/ttyUSB*` node. Without a stable ID, the canonical device path is part of the evidence.

During controlled reconnect, `SessionStateMachine` preserves the logical `SessionId` but accepts the transport only if the new Verified identity has the same device fingerprint, profile ID and profile hash. A changed identity faults the session and closes the replacement transport. Process-level acceptance exercises this behavior by removing a Manual selection path, allowing normal reconnect scheduling to begin, then pointing the same selection path at a second independently allocated simulator PTY. Because a Manual fingerprint includes the verified canonical device path, the replacement PTY produces a different fingerprint and is rejected. The simulator's internal `FingerprintChange` field is not treated as hardware-visible evidence.

### Offline report export

On a non-Match report, `e` writes a versioned JSON report through the storage boundary using create-new semantics. Export uses already retained diagnostics only: it does not reopen the port, rerun identification or transmit Modbus traffic. Repeated exports choose a new filename rather than overwriting an existing report.

## Rendering

The renderer is pure and contains no I/O or sleeping. The composition-root loop coalesces dirty state and schedules frames at the configured `render_fps`, which is validated by #6 to be at most 10 FPS. The minimum supported viewport is 80×24; smaller terminals show only a resize warning.

A resize updates the presentation viewport and increments its `layout_revision`. Layout is recomputed from the current Ratatui frame area, so stale rectangles are never retained across a resize.

The top status block renders application-owned session information: session ID, port, verified profile/profile hash, authorization, audit health, operation and link state. Safety-relevant states contain explicit text (`PROCESS-OFF`, `DISARMED`, `ARMED`, `DEGRADED!`) and therefore do not depend on color.

All nine top-level screens exist as navigation/rendering boundaries. The connection workflow is implemented by #13; dashboard/scope remains #14, parameters/write intents #15, faults #18, and durable diagnostics/audit #22.

## Terminal lifecycle

`TerminalGuard` is the single owner of raw mode, alternate screen and cursor visibility. `restore()` is idempotent and is used by normal shutdown, error/Drop paths, signal shutdown and the composition-root panic hook.

The global panic hook is installed only by `vfd-lantern`. Its first action is terminal restoration. Durable minimal panic-report persistence belongs to #22; #12/#13 establish the ordering and restoration boundary without implementing the later audit/diagnostics storage policy.

SIGINT and SIGTERM are received by the composition root and dispatched as the same `SessionInput::Shutdown` used by `q`/Ctrl+C. Startup may perform passive udev discovery, but it does not open a serial device or transmit Modbus until the user reaches Summary and explicitly chooses Connect.

## Testing and acceptance evidence

Normal workspace tests cover presentation-state reduction, resize invalidation, modal shortcut blocking, profile search, manual path editing, collision-free help, idempotent terminal restoration, no-color safety labels, minimum/undersized TestBackend rendering, connection reducer invariants, stable fingerprints, non-Match rejection, reconnect identity rejection, production PTY open/identification and transport error mapping.

The mandatory CI step `Run connection process E2E` executes separate real `lantern-sim` and `vfd-lantern` processes connected through simulator PTYs. `vfd-lantern` runs under a controlled terminal whose harness answers terminal cursor-position queries and is driven through the same Connection wizard with normal keys. The matrix covers:

- Match with default process write gate (`PROCESS-OFF`);
- Match with `--enable-writes` (`DISARMED`, never `ARMED`);
- Partial;
- Mismatch plus offline report export;
- Ambiguous;
- response timeout;
- Modbus protocol exception;
- controlled Manual-path loss followed by reconnect to a second PTY, where the changed Verified fingerprint must fault the old logical session and close the replacement transport.

The simulator JSONL trace is checked after every process case. It permits only read functions 03/04 and a bounded number of identification requests, providing explicit evidence that the wizard and reconnect verification perform no write and start no hidden telemetry polling. The reconnect case requires exactly one identification read against the original simulator and exactly one against the replacement simulator.

Architecture checks forbid reducer/session state, filesystem access, transport/storage adapters and global panic-hook installation inside `lantern-tui`, and forbid a production dependency from `vfd-lantern` to `lantern-sim`.
