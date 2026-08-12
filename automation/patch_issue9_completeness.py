#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-app/src/session.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "    PortOpened {\n        identity: AdapterIdentity,\n    },",
    "    PortOpened {\n        identity: AdapterIdentity,\n    },\n    PortOpenFailed {\n        cause: SessionFault,\n    },",
)
text = text.replace(
    "    ReconnectPortOpened {\n        identity: AdapterIdentity,\n    },",
    "    ReconnectPortOpened {\n        identity: AdapterIdentity,\n    },\n    ReconnectFailed {\n        cause: SessionFault,\n        now: Instant,\n    },",
)
needle = '''            (
                SessionState::Connecting { .. },
                SessionInput::PortOpened { identity },
            ) => transition(
                SessionState::Identifying {
                    opened_port: identity,
                },
                vec![SessionEffect::StartIdentification],
            ),
'''
addition = needle + '''            (
                SessionState::Connecting { .. },
                SessionInput::PortOpenFailed { .. },
            ) => disconnected(None, Vec::new()),
'''
if needle not in text:
    raise SystemExit("initial PortOpened arm not found")
text = text.replace(needle, addition)
needle = '''            (
                SessionState::Active(active),
                SessionInput::ReconnectPortOpened { .. },
            ) if matches!(&active.connectivity, Connectivity::Reconnecting { .. }) => transition(
                SessionState::Active(active),
                vec![SessionEffect::StartReconnectIdentification],
            ),
'''
addition = needle + '''            (
                SessionState::Active(active),
                SessionInput::ReconnectFailed { cause, now },
            ) if matches!(&active.connectivity, Connectivity::Reconnecting { .. }) => {
                reconnect_failed(active, cause, now, process_writes_enabled)
            }
'''
if needle not in text:
    raise SystemExit("ReconnectPortOpened arm not found")
text = text.replace(needle, addition)
text = text.replace(
    '''                    return transition(SessionState::Active(active), vec![SessionEffect::OpenPort]);''',
    '''                    return transition(
                        SessionState::Active(active),
                        vec![SessionEffect::CancelReconnect, SessionEffect::OpenPort],
                    );''',
)
needle = '''            (SessionState::ShuttingDown, SessionInput::ShutdownComplete) => disconnected(None, Vec::new()),
            (_, SessionInput::Shutdown) => transition(
'''
replacement = '''            (SessionState::ShuttingDown, SessionInput::ShutdownComplete) => disconnected(None, Vec::new()),
            (SessionState::ShuttingDown, SessionInput::Shutdown) => {
                transition(SessionState::ShuttingDown, Vec::new())
            }
            (_, SessionInput::Shutdown) => transition(
'''
if needle not in text:
    raise SystemExit("shutdown arm not found")
text = text.replace(needle, replacement)
insert_before = '''fn same_identity(left: &VerifiedSessionIdentity, right: &VerifiedSessionIdentity) -> bool {'''
helper = '''fn reconnect_failed(
    mut active: ActiveSession,
    cause: SessionFault,
    now: Instant,
    process_writes_enabled: bool,
) -> SessionTransition {
    let attempt = match &active.connectivity {
        Connectivity::Reconnecting { attempt, .. } => attempt.saturating_add(1),
        _ => 0,
    };
    let next_retry_at = now + reconnect_delay(attempt);
    active.connectivity = Connectivity::Reconnecting {
        attempt,
        next_retry_at,
        last_error: cause,
        open_in_progress: false,
    };
    active.authorization = disarmed_for_process(
        process_writes_enabled,
        DisarmReason::TransportLost,
    );
    active.operation = OperationState::Idle;
    transition(
        SessionState::Active(active),
        vec![
            SessionEffect::ClosePort,
            SessionEffect::ScheduleReconnect { at: next_retry_at },
        ],
    )
}

'''
if insert_before not in text:
    raise SystemExit("same_identity helper not found")
text = text.replace(insert_before, helper + insert_before)
# Add two focused regression tests before the final reconnect_backoff test.
needle = '''    #[test]
    fn reconnect_backoff_is_capped() {'''
tests = '''    #[test]
    fn failed_reconnect_advances_the_bounded_backoff() {
        let mut machine = active_machine();
        let now = Instant::now();
        machine.transition(SessionInput::TransportLost {
            cause: SessionFault::PortRemoved,
            now,
        });
        let effects = machine.transition(SessionInput::ReconnectFailed {
            cause: SessionFault::Transport(BusError::ResponseTimeout),
            now,
        });
        assert_eq!(
            effects,
            vec![
                SessionEffect::ClosePort,
                SessionEffect::ScheduleReconnect {
                    at: now + Duration::from_millis(500),
                },
            ]
        );
    }

    #[test]
    fn repeated_shutdown_is_idempotent() {
        let mut machine = active_machine();
        assert!(!machine.transition(SessionInput::Shutdown).is_empty());
        assert!(machine.transition(SessionInput::Shutdown).is_empty());
    }

    #[test]
    fn reconnect_backoff_is_capped() {'''
if needle not in text:
    raise SystemExit("backoff test marker not found")
text = text.replace(needle, tests)
# Tests need BusError.
text = text.replace(
    "    use crate::{AdapterIdentity, AuditHealth, Authorization, Connectivity, DisarmReason, OperationState};",
    "    use crate::{AdapterIdentity, AuditHealth, Authorization, BusError, Connectivity, DisarmReason, OperationState};",
)

# Keep the canonical state compact while retaining value semantics inside the reducer.
text = text.replace("    Active(ActiveSession),", "    Active(Box<ActiveSession>),")
old = '''                if report.outcome == IdentificationMatch::Match {
                    if let Some(identity) = verified {
                        let authorization = if process_writes_enabled {
                            Authorization::Disarmed {
                                reason: DisarmReason::Initial,
                            }
                        } else {
                            Authorization::ProcessDisabled
                        };
                        return transition(
                            SessionState::Active(ActiveSession {
                                session_id,
                                identity,
                                port_identity: opened_port,
                                connectivity: Connectivity::Connected,
                                authorization,
                                audit_health: AuditHealth::Healthy,
                                operation: OperationState::Idle,
                            }),
                            Vec::new(),
                        );
                    }
                }
'''
new = '''                if report.outcome == IdentificationMatch::Match
                    && let Some(identity) = verified
                {
                    let authorization = if process_writes_enabled {
                        Authorization::Disarmed {
                            reason: DisarmReason::Initial,
                        }
                    } else {
                        Authorization::ProcessDisabled
                    };
                    return transition(
                        SessionState::Active(Box::new(ActiveSession {
                            session_id,
                            identity,
                            port_identity: opened_port,
                            connectivity: Connectivity::Connected,
                            authorization,
                            audit_health: AuditHealth::Healthy,
                            operation: OperationState::Idle,
                        })),
                        Vec::new(),
                    );
                }
'''
if old not in text:
    raise SystemExit("initial verified identification gate not found")
text = text.replace(old, new)
text = text.replace(
    "            ) => transport_lost(active, cause, now),",
    "            ) => transport_lost(*active, cause, now),",
)
text = text.replace(
    "                transport_lost(active, SessionFault::PortRemoved, now)",
    "                transport_lost(*active, SessionFault::PortRemoved, now)",
)
text = text.replace(
    "                reconnect_failed(active, cause, now, process_writes_enabled)",
    "                reconnect_failed(*active, cause, now, process_writes_enabled)",
)
text = text.replace(
    "    transition(SessionState::Active(active), effects)\n}\n\nfn reconnect_failed(",
    "    transition(SessionState::Active(Box::new(active)), effects)\n}\n\nfn reconnect_failed(",
)
text = text.replace(
    "        SessionState::Active(active),\n        vec![\n            SessionEffect::ClosePort,",
    "        SessionState::Active(Box::new(active)),\n        vec![\n            SessionEffect::ClosePort,",
)
path.write_text(text, encoding="utf-8")

# Keep the crate root as the stable public API surface after adding the new modules.
path = Path("crates/lantern-app/src/lib.rs")
text = path.read_text(encoding="utf-8")
exports = [
    "pub use application::*;",
    "pub use bus::*;",
    "pub use ports::*;",
    "pub use profile_registry::*;",
    "pub use serial::*;",
    "pub use session::*;",
    "pub use settings::*;",
    "pub use write_coordinator::*;",
]
for export in exports:
    text = text.replace(export + "\n", "")
anchor = "mod write_coordinator;\n"
if anchor not in text:
    raise SystemExit("lantern-app module block not found")
text = text.replace(anchor, anchor + "\n" + "\n".join(exports) + "\n", 1)
path.write_text(text, encoding="utf-8")
