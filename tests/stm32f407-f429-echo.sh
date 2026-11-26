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
    --max-steps "${PENV_ECHO_ROUNDS:-5000000}" --stats \
    > "$work/uart.log" 2> "$work/run.log" || status=$?
printf '%s\n' "$status" > "$work/run.status"

# The shared stdout can interleave bytes from both UARTs. Check observable
# TCP progress here; Robot equivalence requires per-node timed assertions.
if [[ "$status" != 2 ]] || ! grep -qF 'simulation round limit reached' "$work/run.log"; then
    cat "$work/run.log" >&2
    exit 1
fi
for marker in 'Initializing network' 'Run echo server' 'Run echo client' 'Accepted connection'; do
    if ! grep -aqF "$marker" "$work/uart.log"; then
        printf 'BLOCKED: %s was absent from the combined UART log\n' "$marker" >&2
        exit 1
    fi
done
matches=$(grep -acE 'IPv4 TCP: Received and compared [0-9]+ bytes, all ok' "$work/uart.log" || :)
if [[ "$matches" == 0 ]]; then
    printf '%s\n' 'BLOCKED: no intact TCP compare line in combined UART' >&2
    exit 1
fi
printf 'PARTIAL: TCP connection and %s intact TCP compare lines; per-node Robot assertions pending\n' "$matches"
