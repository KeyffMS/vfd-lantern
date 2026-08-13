# ADR 0001: Modular monolith and dependency inversion

- Status: accepted
- Scope: VFD Lantern 1.0

## Decision

VFD Lantern is one process and one production binary assembled by `vfd-lantern`.
Application policy lives in `lantern-app`. The application crate defines narrow outbound
ports; `lantern-storage` and `lantern-transport` implement them. Presentation lives in
`lantern-tui` and receives read-only view models. `lantern-sim` is development-only.

The allowed dependency direction is:

```text
lantern-domain <- lantern-profile <- lantern-app
                                      ^   ^   ^
                                      |   |   |
                                storage transport tui
                                      \   |   /
                                     vfd-lantern

lantern-sim -> domain + profile + app + transport
```

## Sources of truth and authorities

- Profile: `ValidatedDeviceProfile`.
- Application state: `ApplicationState`.
- Presentation state: `UiState`.
- Profile registry: `ProfileRegistry`.
- Polling policy: `PollPlanner`.
- Bus authority: one future `BusActor`.
- Write authority: one future `WriteCoordinator`.
- File ownership: storage adapters.

## Consequences

There are no runtime plugins, scripting engines, service locators, global mutable
singletons, or adapter imports in the TUI. The production binary is the only composition
root. Every new dependency edge must remain acyclic and is checked in CI.
