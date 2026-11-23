#!/bin/sh
# Ports the audio, sensor, Shell, watchdog reset, and interrupt cases from NRF52840.robot.
set -eu

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
renode=$2
artifacts=$3
test_dir=$(dirname "$0")
mkdir -p "$artifacts"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

asset() {
    name=$1
    remote=$2
    destination=$artifacts/$name
    if [ ! -f "$destination" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$destination.part"
        mv "$destination.part" "$destination"
    fi
    printf '%s  %s\n' "${remote##*-}" "$destination" | sha1sum --check --status
}

asset speech.elf nrf52840--tflite-micro_speech.elf-s_7172308-9ab5781883d7af2582d7fea09b14352628da9839
asset yes.pcm audio_yes_1s.s16le.pcm-s_32000-b69f5518615516f80ae0082fe9b5a5d29ffebce8
asset i2s.elf nrf52840--nordic_snippets_i2s_master.elf-s_181224-98d5e53081cf7d76d30a183978a03b3e00beaf53
asset uart.elf zephyr_shell_nrf52840.elf-s_1110556-9653ab7fffe1427c50fa6b837e55edab38925681
asset uarte.elf renode-nrf52840-zephyr_shell_module.elf-gf8d05cf-s_1310072-c00fbffd6b65c6238877c4fe52e8228c2a38bf1f
asset watchdog.elf nrf52840--zephyr-watchdog.elf-s_889852-75a696a301c8439400645e646a322039963d74c8
asset task-wdt.elf nrf52840--zephyr-task_wdt.elf-s_904904-91eadd958664ba0a009403a1a64f429dce3603a3
asset i2c.elf nrf52840--zephyr_adxl372_i2c.elf-s_944004-aacf7d772ebcc5a26c156f78ebdef2e03f803cc3
asset spi.elf nrf52840--zephyr_adxl372_spi.elf-s_993780-1dedb945dae92c07f1b4d955719bfb1f1e604173
asset wfe.elf zephyr-custom_wfe_test.elf-s_601916-bffaab1a16e3cb3c357701584be101e447e4e2ab
asset interrupt.elf nrf52840-zephyr-c320bb0-kernel-interrupt-test.elf-s_737512-7802727d11c415c1859c00842063ebec6c0a6fdc

run() {
    log=$1
    shift
    status=0
    "$penv" run --platform-root "$renode" "$@" > "$artifacts/$log.log" 2>&1 || status=$?
    if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
        cat "$artifacts/$log.log" >&2
        exit "$status"
    fi
}

repl=$renode/platforms/cpus/nrf52840.repl
check_shell() (
    printf 'demo ping\r' > "$work/ping"
    cat > "$work/uart.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
uart0:
    easyDMA: false
uart1:
    easyDMA: false
REPL
    run shell-uart --platform "$work/uart.repl" --max-steps 700000 \
        --uart-event "100000:$work/ping" --reset-at 300000 --uart-event "400000:$work/ping" "$artifacts/uart.elf"
    awk '
        /uart:~[$]/ { prompt = 1 }
        /demo ping/ { if (!prompt || command) exit 1; command = 1 }
        /^pong\r?$/ {
            if (!command) exit 1
            replies++
            prompt = command = 0
        }
        END { if (replies != 2 || command) exit 1 }
    ' "$artifacts/shell-uart.log"
    "$test_dir/nrf52840-shell.sh" "$penv" "$renode" "$artifacts/uarte.elf" > "$artifacts/shell-uarte.log" 2>&1
)

check_watchdog() (
    run watchdog --platform "$repl" --max-steps 130000000 "$artifacts/watchdog.elf"
    test "$(grep -ac '^Watchdog sample application' "$artifacts/watchdog.log")" -ge 2
    grep -aFq 'Waiting for reset...' "$artifacts/watchdog.log"
)

check_task_watchdog() (
    run task-watchdog --platform "$repl" --max-steps 1000000000 "$artifacts/task-wdt.elf"
    awk '
        /^Task watchdog sample application/ { starts++ }
        /^Task watchdog channel 1 callback, thread: control/ && starts > callbacks { callbacks++ }
        /^Resetting device/ && callbacks > resets { resets++ }
        END { exit !(resets >= 2 && starts > resets) }
    ' "$artifacts/task-watchdog.log"
)

check_irq() (
    run wfe --platform "$repl" --max-steps 15000000 "$artifacts/wfe.elf"
    grep -aFq 'First WFE should finish' "$artifacts/wfe.log"
    "$test_dir/nrf52840-kernel-interrupt.sh" "$penv" "$renode" "$artifacts/interrupt.elf" > "$artifacts/interrupt.log" 2>&1
)

check_i2s() (
    printf '\000\000\000\000\132\202\132\202\177\377\177\377\132\202\132\202\000\000\000\000\245\176\245\176\200\000\200\000\245\176\245\176' > "$artifacts/i2s-input.pcm"
    run i2s --platform "$repl" --max-steps 1000000 --i2s-input "$artifacts/i2s-input.pcm" \
        --i2s-output "$artifacts/i2s-output.pcm" "$artifacts/i2s.elf"
    cmp "$artifacts/i2s-input.pcm" "$artifacts/i2s-output.pcm"
)

check_sensor() (
    bus=$1
    shift
    case $bus in
        i2c) registration='twi1 0x11' ;;
        spi) registration=spi2 ;;
    esac
    board=$work/$bus-$4.repl
    cat > "$board" <<REPL
using "platforms/cpus/nrf52840.repl"
adxl372: Sensors.ADXL372 @ $registration
    accelerationX: $1
    accelerationY: $2
    accelerationZ: $3
REPL
    if [ "$bus" = spi ]; then
        printf 'gpio0:\n    22 -> adxl372@0\n' >> "$board"
    fi
    run "$bus-$4" --platform "$board" --max-steps 130000000 "$artifacts/$bus.elf"
    if ! grep -aFq " $4 g:" "$artifacts/$bus-$4.log"; then
        cat "$artifacts/$bus-$4.log" >&2
        exit 1
    fi
)

check_speech() {
    "$test_dir/nrf52840-pdm-upstream.sh" "$penv" "$renode" "$artifacts/speech.elf" "$artifacts/yes.pcm" \
        > "$artifacts/speech.log" 2>&1
}

pids=
for check in check_shell check_watchdog check_task_watchdog check_irq check_i2s check_speech; do
    "$check" &
    pids="$pids $!"
done
for bus in i2c spi; do
    for sample in '0 0 0 0.00' '1 0 0 1.00' '2 2 0 2.83' '3 3 3 5.20'; do
        set -- $sample
        check_sensor "$bus" "$@" &
        pids="$pids $!"
    done
done

failed=0
for pid in $pids; do
    wait "$pid" || failed=1
done
if [ "$failed" -ne 0 ]; then
    echo "An nRF52840 upstream case failed. See logs in $artifacts" >&2
    exit 1
fi
printf 'nRF52840 upstream audio and peripheral cases passed\n'
