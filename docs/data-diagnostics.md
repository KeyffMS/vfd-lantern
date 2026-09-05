# CSV, diagnostics and recovery artifacts

CSV logging uses a bounded non-blocking queue and a single storage actor. Telemetry is written in long/tidy UTF-8 CSV; queue pressure creates explicit gap records instead of blocking Modbus polling. The final portable `<csv>.session.json` sidecar stays beside the CSV, while a separate runtime checkpoint lives in XDG state and is removed after a clean finalization.

Fault events remain valid even when freeze-frame collection is partial or fails. See the dedicated faults chapter for scalar/bitset transitions and bus-priority rules.

Diagnostic logs, durable audit, panic reports and interrupted-runtime evidence live under XDG state. Portable backups, CSV files, fault reports and explicitly collected diagnostic bundles live under XDG data or an operator-selected output directory.

Diagnostic bundles are bounded and explicit about whether device values, CSV, backups, fault reports, profile data or audit evidence are included. Collection must not silently turn on writes, reconnect the device or elevate an untrusted profile.

For recovery after an interrupted write/restore, trust durable audit and fresh device reads rather than stale UI state. Restore is never automatically resumed; construct a new backup/diff/plan if another attempt is required.
