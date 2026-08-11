#!/usr/bin/env python3
from pathlib import Path

root = Path.cwd()

manifest = root / "crates/lantern-transport/Cargo.toml"
text = manifest.read_text(encoding="utf-8")
if "[dev-dependencies]" not in text:
    text += "\n[dev-dependencies]\ntempfile.workspace = true\n"
elif "tempfile.workspace = true" not in text:
    text = text.replace("[dev-dependencies]\n", "[dev-dependencies]\ntempfile.workspace = true\n", 1)
manifest.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/discovery.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "descriptor_from_device(event.device(), presence, &stable_links)",
    "descriptor_from_device(&event, presence, &stable_links)",
)
text = text.replace(
    "links.sort_by(|left, right| left.1.cmp(&right.1));\n    links.into_iter().collect()",
    "links.sort_by(|left, right| left.1.cmp(&right.1));\n    let mut result = BTreeMap::new();\n    for (target, link) in links {\n        result.entry(target).or_insert(link);\n    }\n    result",
)
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/serial_open.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "if matches!(error.raw_os_error(), Some(libc::ENOTTY | libc::EINVAL)) {",
    "if matches!(error.raw_os_error(), Some(libc::ENOTTY) | Some(libc::EINVAL)) {",
)
text = text.replace(
    "let error = SerialPortOpener::open(&request(file.path().to_path_buf()))\n            .expect_err(\"regular file must fail\");\n        assert!(matches!(\n            error,\n            lantern_app::SerialConnectError::NotCharacterDevice { .. }\n        ));",
    "let result = SerialPortOpener::open(&request(file.path().to_path_buf()));\n        assert!(matches!(\n            result,\n            Err(lantern_app::SerialConnectError::NotCharacterDevice { .. })\n        ));",
)
path.write_text(text, encoding="utf-8")
