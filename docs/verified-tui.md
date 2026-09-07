# Verified connection and TUI

A new process starts disconnected and unarmed. Configuration errors are handled before the serial port is opened. The connection workflow resolves the adapter and link settings, probes the candidate device and profile, and creates a verified session only after identity is unambiguous.

A verified session binds a `SessionId`, device fingerprint, profile hash, slave address and link context. Reconnect to the same identity can preserve session continuity, but arming never survives reconnect. A different fingerprint cannot inherit the previous session's write authority.

The Ratatui interface consumes application state; it does not implement its own Modbus path. Parameter browsing, monitoring, scope, faults and write interactions use the same domain/profile models as CLI and tests. Quantity and unit formatting are presentation concerns: safety decisions use typed engineering values and authoritative raw registers from the validated profile.

Write interactions are deliberately two-stage. The UI may show a preview, but the final target, old value, guards and drive state are re-resolved by `WriteCoordinator`. Stale UI state cannot authorize a write.
