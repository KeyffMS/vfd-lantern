#!/bin/sh
set -eu

internal='lantern-domain|lantern-profile|lantern-app|lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern'

assert_absent() {
    package="$1"
    forbidden="$2"
    tree="$(cargo tree --locked -p "$package" --depth 1 --edges normal --prefix none)"
    if printf '%s\n' "$tree" | grep -Eq "^(${forbidden})( |$)"; then
        printf 'forbidden dependency in %s: %s\n' "$package" "$forbidden" >&2
        exit 1
    fi
}

assert_absent lantern-domain 'lantern-profile|lantern-app|lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|tokio|serde|ratatui|crossterm|udev|nix|libc'
assert_absent lantern-profile 'lantern-app|lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|tokio|ratatui|crossterm|udev|nix|libc'
assert_absent lantern-app 'lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|ratatui|crossterm|udev|nix|libc'
assert_absent lantern-storage 'lantern-profile|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|ratatui|crossterm|udev'
assert_absent lantern-transport 'lantern-profile|lantern-storage|lantern-tui|lantern-sim|vfd-lantern|ratatui|crossterm'
assert_absent lantern-sim 'lantern-storage|lantern-tui|vfd-lantern|ratatui|crossterm|udev'
assert_absent lantern-tui 'lantern-domain|lantern-profile|lantern-storage|lantern-transport|lantern-sim|vfd-lantern|tokio-modbus|tokio-serial|udev|nix|libc'
assert_absent vfd-lantern 'lantern-sim'

if grep -R -n -E 'std::fs|tokio::fs' crates/lantern-profile/src crates/lantern-domain/src crates/lantern-tui/src; then
    printf 'filesystem access found outside storage/composition boundaries\n' >&2
    exit 1
fi

if grep -R -n -E '\b(ApplicationState|ApplicationRuntime|EffectRunner|SessionStateMachine)\b' crates/lantern-tui/src; then
    printf 'application/domain reducer state escaped into lantern-tui\n' >&2
    exit 1
fi

if grep -R -n -E 'panic::set_hook|set_hook[[:space:]]*\(' crates/lantern-tui/src; then
    printf 'global panic hook must not be installed by lantern-tui\n' >&2
    exit 1
fi

if ! grep -R -n -E 'panic::set_hook|set_hook[[:space:]]*\(' crates/vfd-lantern/src >/dev/null; then
    printf 'composition root must own the global panic hook\n' >&2
    exit 1
fi

if find . -type f -name '*.py' -not -path './target/*' | grep -q .; then
    printf 'Python source is not permitted in the project\n' >&2
    exit 1
fi

if grep -R -n -E 'unsafe[[:space:]]*\{|unsafe[[:space:]]+(fn|impl|trait)' crates --include='*.rs' \
    | grep -v '^crates/lantern-transport/src/rs485_ioctl.rs:'; then
    printf 'project-owned unsafe code exists outside the isolated RS-485 ioctl module\n' >&2
    exit 1
fi

if grep -n -E 'open_native_async|SerialPortOpener|OpenOptions|File::open|libc::open' \
    crates/lantern-transport/src/discovery.rs; then
    printf 'passive discovery contains a serial-device open path\n' >&2
    exit 1
fi

for manifest in crates/*/Cargo.toml; do
    case "$manifest" in
        crates/lantern-transport/Cargo.toml|crates/lantern-sim/Cargo.toml) ;;
        *)
            if grep -Eq '^(udev|tokio-serial|nix)\.workspace' "$manifest"; then
                printf 'Linux serial dependency escaped the transport adapter: %s\n' "$manifest" >&2
                exit 1
            fi
            ;;
    esac
done

if ! grep -R -n -E '\bWriteCoordinator\b' crates/vfd-lantern/src/write_runtime.rs >/dev/null; then
    printf 'issue #23 requires the production composition root to instantiate WriteCoordinator\n' >&2
    exit 1
fi

if awk '/#\[cfg\(test\)\]/{exit} {print}' crates/vfd-lantern/src/write_runtime.rs \
        | grep -n -E '\bPreparedBusWrite\b'; then
    printf 'production write composition must never mint or expose PreparedBusWrite directly\n' >&2
    exit 1
fi

if find crates/vfd-lantern/src -type f -name '*.rs' ! -name 'write_runtime.rs' -print0 \
        | xargs -0 grep -n -E '\bPreparedBusWrite\b'; then
    printf 'PreparedBusWrite escaped the guarded write runtime boundary\n' >&2
    exit 1
fi

if ! grep -n -E '\bFilesystemAuditPort\b' crates/vfd-lantern/src/write_runtime.rs >/dev/null \
    || ! grep -n -E '\bRuntimeProfileTrust\b' crates/vfd-lantern/src/write_runtime.rs >/dev/null; then
    printf 'production guarded writes require both durable audit and runtime profile trust adapters\n' >&2
    exit 1
fi

if grep -R -n -E '\b(TcpStream|TcpListener|UdpSocket|reqwest|hyper|ureq)\b' \
    crates/lantern-app/src crates/lantern-storage/src crates/vfd-lantern/src; then
    printf 'network endpoint/client path found in application, storage, or composition root\n' >&2
    exit 1
fi

cargo metadata --locked --no-deps --format-version 1 >/dev/null
if ! grep -q 'SessionInput::ArmWrites' crates/lantern-tui/src/parameter_keymap.rs \
    || ! grep -q 'ParameterAction::PrepareWrite' crates/lantern-tui/src/parameter_keymap.rs \
    || ! grep -q 'ParameterAction::ConfirmPrepared' crates/lantern-tui/src/keymap.rs; then
    printf 'issue #23 requires explicit arming, prepare and phase-2 confirmation in the TUI boundary\n' >&2
    exit 1
fi

if [ ! -f docs/development/threat-model.md ]; then
    printf 'issue #23 requires an explicit industrial threat model\n' >&2
    exit 1
fi

if ! grep -q 'missing_audit_adapter_never_mints_write_capability_or_touches_bus' crates/vfd-lantern/src/write_runtime.rs \
    || ! grep -q 'missing_profile_trust_adapter_never_mints_write_capability_or_touches_bus' crates/vfd-lantern/src/write_runtime.rs; then
    printf 'issue #23 requires fail-closed composition tests for missing audit/trust adapters\n' >&2
    exit 1
fi

if grep -q 'operator_text\.trim()' crates/lantern-app/src/application.rs; then
    printf 'phase-2 operator confirmation must be exact; whitespace normalization is forbidden\n' >&2
    exit 1
fi

printf 'architecture checks passed\n'
