#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-app/src/session.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "    WriteFinished {\n        outcome: WriteOutcome,\n    },",
    "    WriteFinished {\n        outcome: WriteOutcome,\n        now: Instant,\n    },",
)
text = text.replace(
    "                SessionInput::WriteFinished { outcome },",
    "                SessionInput::WriteFinished { outcome, now },",
)
text = text.replace("                            since: Instant::now(),", "                            since: now,")
text = text.replace("    let attempt = match active.connectivity {", "    let attempt = match &active.connectivity {")
path.write_text(text, encoding="utf-8")
