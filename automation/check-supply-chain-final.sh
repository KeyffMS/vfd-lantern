#!/bin/sh
set -eu

sh scripts/check-supply-chain-baseline.sh
cargo machete
cargo deny check
cargo audit --file Cargo.lock
cargo vet check
