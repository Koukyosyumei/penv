#!/usr/bin/env bash
# STM32H7 configurations of Try Load Platform in platform_descriptions.robot.
set -eu

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit states}
mkdir -p "$logs"

failed=0
for platform in boards/nucleo_h753zi boards/stm32h747-disco boards/stm32h7_renode_reference_board \
    cpus/stm32h7 cpus/stm32h743 cpus/stm32h747 cpus/stm32h753; do
    name=${platform##*/}
    status=0
    # Each invocation creates and discards a fresh machine, like Reset Emulation.
    "$penv" check-platform "$renode/platforms/$platform.repl" --platform-root "$renode" \
        > "$logs/check-$name.log" 2>&1 || status=$?
    printf '%s\n' "$status" > "$logs/check-$name.exit"
    printf '%s %s\n' "$status" "$platform.repl"
    if (( status != 0 )); then
        failed=1
    fi
done
exit "$failed"
