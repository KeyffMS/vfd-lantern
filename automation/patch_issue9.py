#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-app/src/session.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "    DeviceFingerprint, IdentificationMatch, IdentificationReport, OperationId, PlanId, ProfileId,\n",
    "    IdentificationMatch, IdentificationReport, OperationId, PlanId,\n",
)
text = text.replace(
    "    ConfirmArming {\n        challenge: String,\n        idle_expires_at: Instant,\n    },",
    "    ConfirmArming {\n        challenge: String,\n        now: Instant,\n        idle_expires_at: Instant,\n    },",
)
text = text.replace(
    "                SessionInput::ConfirmArming {\n                    challenge,\n                    idle_expires_at,\n                },",
    "                SessionInput::ConfirmArming {\n                    challenge,\n                    now,\n                    idle_expires_at,\n                },",
)
text = text.replace("                    && Instant::now() <= *expires_at", "                    && now <= *expires_at")
text = text.replace(
    "    let next_retry_at = now + reconnect_delay(0);\n    active.connectivity = Connectivity::Reconnecting {\n        attempt: 0,",
    "    let attempt = match active.connectivity {\n        Connectivity::Reconnecting { attempt, .. } => attempt.saturating_add(1),\n        _ => 0,\n    };\n    let next_retry_at = now + reconnect_delay(attempt);\n    active.connectivity = Connectivity::Reconnecting {\n        attempt,",
)
path.write_text(text, encoding="utf-8")
