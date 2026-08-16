# Deterministic PTY/Modbus RTU simulator

`lantern-sim` is a development-only process. It exposes a real Linux slave PTY,
runs the pinned `tokio-modbus` RTU server, and never opens a network listener.
It is not included in user packages and does not expose a write policy.

Run the versioned example scenario from the workspace root:

```sh
cargo run -p lantern-sim --locked -- \
  --profile profiles/example-vfd.toml \
  --scenario scenarios/example-read-only.toml
```

The first stdout line is a JSON handshake containing the slave PTY path, exact
profile hash, scenario hash, fingerprint, and 256-bit deterministic seed. The
product-side integration must open that PTY through `open_serial_bus`; direct
probe clients are not an acceptance path.

By default, responses travel directly from
`tokio_modbus::server::rtu::Server` through the PTY. A scenario containing
`wire_faults` creates a separate byte proxy that may corrupt only scheduled
response frames. The proxy has no register map, profile parser, or product-side
test mode.

The scenario is a closed TOML document. It is bound to the exact normalized
`profile_hash`, uses the profile's validated register blocks and codecs, and
rejects unknown fields and parameter identifiers. Runtime traces are written as
JSON Lines and include semantic request/response payloads and deterministic
scenario metadata.
