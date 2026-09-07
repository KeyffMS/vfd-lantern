#!/bin/sh
set -eu
cargo bench --locked -p lantern-sim --bench infrastructure -- --test
# Retain the product budgets already enforced before #21.
cargo run --locked --release -p lantern-app --example telemetry_pipeline_benchmark
cargo run --locked --release -p lantern-tui --example scope_render_benchmark
cargo run --locked --release -p lantern-tui --example parameter_browser_benchmark
