# TUI architecture

`lantern-tui` is a presentation adapter. It owns terminal presentation state and Ratatui rendering, but it does not own application/domain state, transport adapters, storage adapters, a session reducer or an effect runner.

## One-way state flow

1. Crossterm `EventStream` produces a terminal event.
2. The keymap maps it to either a `UiAction` or an application-owned `ApplicationAction`.
3. `UiAction` mutates only `UiState` (screen, focus, scroll, selection, form, modal and viewport).
4. `ApplicationAction` is dispatched through `ApplicationRuntime`; effects remain application-owned and are executed at the composition root.
5. Rendering reads only immutable `ApplicationView` and `UiState` snapshots.

The TUI never opens serial devices, reads files or creates a second telemetry/session state.

## Rendering

The renderer is pure and contains no I/O or sleeping. The composition-root loop coalesces dirty state and schedules frames at the configured `render_fps`, which is validated by #6 to be at most 10 FPS. The minimum supported viewport is 80×24; smaller terminals show only a resize warning.

A resize updates the presentation viewport and increments its `layout_revision`. Layout is recomputed from the current Ratatui frame area, so stale rectangles are never retained across a resize.

The top status block renders application-owned session information: session ID, port, verified profile/profile hash, authorization, audit health, operation and link state. Slave selection is intentionally shown as unavailable until the Verified-only connection workflow in #13 owns that value. Safety-relevant states contain explicit text (`DISARMED`, `ARMED`, `DEGRADED!`) and therefore do not depend on color.

All nine top-level screens exist in #12 as navigation/rendering boundaries. Feature content remains with its roadmap owner: connection wizard #13, dashboard/scope #14, parameters #15, faults #18, CSV/backup-related workflows in their respective issues, and durable diagnostics/audit #22.

## Terminal lifecycle

`TerminalGuard` is the single owner of raw mode, alternate screen and cursor visibility. `restore()` is idempotent and is used by normal shutdown, error/Drop paths, signal shutdown and the composition-root panic hook.

The global panic hook is installed only by `vfd-lantern`. Its first action is terminal restoration. Durable minimal panic-report persistence belongs to #22, which explicitly depends on #12; #12 establishes the ordering and restoration boundary without implementing the later audit/diagnostics storage policy.

SIGINT and SIGTERM are received by the composition root and dispatched as the same `SessionInput::Shutdown` used by `q`/Ctrl+C. A clean #12 start does not scan ports, open a serial device or transmit Modbus traffic.

## Testing

The TUI tests cover presentation-state reduction, resize invalidation, modal shortcut blocking, collision-free key help, idempotent terminal restoration, no-color safety labels, minimum/undersized TestBackend rendering and panic cleanup ordering. Architecture checks forbid application reducer state, filesystem access, transport/storage adapters and global panic-hook installation inside `lantern-tui`.
