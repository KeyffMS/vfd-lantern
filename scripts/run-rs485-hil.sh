#!/bin/sh
set -eu

: "${VFD_LANTERN_RS485_HIL_DEVICE:?set this to a native UART supporting TIOCGRS485/TIOCSRS485}"

cargo test --locked -p lantern-transport --lib \
    serial_open::tests::kernel_rs485_hil -- \
    --ignored --exact --nocapture
