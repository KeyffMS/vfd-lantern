# Architecture, simulator and CI

VFD Lantern separates pure domain/profile logic from application orchestration, storage, transport and TUI adapters. Ports are owned by the application layer; concrete filesystem/serial implementations point inward rather than allowing UI code to open files or send raw Modbus directly.

Safety-sensitive behavior follows single points of authority: `WriteCoordinator` for physical writes, validated profiles for register meaning, the application poll planner for bus scheduling, and durable audit for write/operation evidence.

The simulator and PTY harness exercise the production serial/RTU boundary. Core scenarios cover connect/read/errors/reconnect; the conformance matrix extends this to faults, CSV, write/audit/trust/restore and pressure cases. Communication scenarios must use the real PTY/BusActor path rather than replacing RTU with mocks.

CI pins Rust, actions and project tools. Fast gates cover formatting, Clippy, docs, tests, profile validation, architecture and supply chain. Reusable long-run infrastructure separates the long execution from a fresh-token upload job and binds staging data to workflow run ID and tested artifact hash.

Release workflow #24 consumes these contracts but does not fabricate missing conformance/HIL evidence. Actual 1.0 candidate gates and public publication belong to the candidate release procedure.
