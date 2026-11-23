#!/usr/bin/env bash
# Reproduce the fixed Renode F429 Zephyr echo case on two Discovery machines.
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-f429-echo}
renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"

server=nucleo_f429zi-zephyr-echo_server.elf-s_3529768-a44aca7749d2850302350f4c2fb4647aecaa8c72
client=nucleo_f429zi-zephyr-echo_client.elf-s_3478872-614b8752844ac17e9335fcd12ad9fcec742571d5
for image in "$server" "$client"; do
    if [[ ! -f "$work/$image" ]]; then
        curl --fail --location --output "$work/$image.part" \
            "https://dl.antmicro.com/projects/renode/$image"
        mv "$work/$image.part" "$work/$image"
    fi
    printf '%s  %s\n' "${image##*-}" "$work/$image" | sha1sum --check -
done

{
    printf '%s\n' 'emulation CreateSwitch "switch"'
    for image in "$server" "$client"; do
        cat <<RESC
mach create
machine LoadPlatformDescription @platforms/boards/stm32f4_discovery-kit.repl
sysbus LoadELF @$image
connector Connect sysbus.ethernet switch
showAnalyzer sysbus.usart3
RESC
    done
    printf '%s\n' start
} > "$work/boot.resc"

status=0
"$penv" run --script "$work/boot.resc" --platform-root "$renode" \
    --max-steps "${PENV_ECHO_ROUNDS:-1000000}" --stats \
    > "$work/uart.log" 2> "$work/run.log" || status=$?
printf '%s\n' "$status" > "$work/run.status"

# Check the first Robot expectation. The CLI currently combines node UART
# output; the complete case still needs per-node timed terminal assertions.
if ! grep -qF 'Initializing network' "$work/uart.log"; then
    printf '%s\n' 'BLOCKED: Initializing network was absent from the combined UART log'
    cat "$work/run.log"
    exit 1
fi
printf '%s\n' 'PARTIAL: Initializing network observed; per-node Robot expectations pending'
