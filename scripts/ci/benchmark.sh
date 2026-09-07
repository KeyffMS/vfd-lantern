#!/bin/sh
set -eu
cargo bench --locked -p lantern-sim --bench infrastructure -- --test
