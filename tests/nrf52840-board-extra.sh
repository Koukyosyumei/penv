#!/bin/sh
# Additional board cases from tests/platforms/NRF52840.robot.
set -eu

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
renode=$2
artifacts=$3
mkdir -p "$artifacts"

asset() {
    path="$artifacts/$1"
    remote=$2
    if [ ! -f "$path" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$path.part"
        mv "$path.part" "$path"
    fi
    printf '%s  %s\n' "${remote##*-}" "$path" | sha1sum --check
}

asset alarm.elf zephyr_alarm_nRF52840.elf-s_489392-49a2ec3fda2f0337fe72521f08e51ecb0fd8d616
asset button.elf nrf52840--zephyr_button.elf-s_660440-50c3b674193c8105624dae389420904e2036f9c0
asset q15.elf nrf52840--zephyr-cmsis_transform_rq15.elf-s_1115276-3e1fb95c3d1283d1fe22acf4b8ef887d05455fdf
asset micropython.elf zephyr-nrf52840--micropython-with-i2c.elf-s_5843396-3434fb85ea0bb234b73af44dcbc4318903a5461b

cat >"$artifacts/alarm.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
uart0:
    easyDMA: false
uart1:
    easyDMA: false
REPL
cat >"$artifacts/button.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
gpio0:
    13 -> led@0
button: Miscellaneous.Button @ gpio0 11
    invert: true
    -> gpio0@11
led: Miscellaneous.LED @ gpio0 13
REPL
cat >"$artifacts/bme.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
bme280: I2C.BME280 @ twi0 0x76
REPL
printf 'import machine\r' >"$artifacts/import.in"
printf 'i = machine.I2C("i2c0")\r' >"$artifacts/init.in"
printf 'print("ID: 0x" + (i.readfrom_mem(0x76, 0xD0, 1).hex() or "00"))\r' >"$artifacts/chip.in"
printf 'print("S:", i.scan())\r' >"$artifacts/scan.in"

run() {
    name=$1
    shift
    status=0
    "$penv" run --platform-root "$renode" "$@" >"$artifacts/$name.log" 2>&1 || status=$?
    if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
        cat "$artifacts/$name.log" >&2
        return "$status"
    fi
}

check_alarm() (
    run alarm --platform "$artifacts/alarm.repl" --uart-steps --max-steps 610000000 "$artifacts/alarm.elf"
    awk '/!!! Alarm !!!/ {
        step = $2 + 0
        if (++seen == 1 && step >= 200000000 && step < 300000000) first = 1
        if (seen == 2 && step >= 600000000 && step < 700000000) second = 1
    }
    END { exit !(seen == 2 && first && second) }' "$artifacts/alarm.log"
)

check_button() (
    run button --platform "$artifacts/button.repl" --led-events \
        --button-event 5000000:button:press --button-event 105000000:button:release \
        --max-steps 205000001 "$artifacts/button.elf"
    awk '/Booting Zephyr OS/ { boot = NR }
        /Press the button/ { prompt = NR }
        /^LED led / { events = events " " $3 }
        END { exit !(boot && boot < prompt && events == " on off on") }' "$artifacts/button.log"
)

check_q15() (
    run q15 --platform "$renode/platforms/cpus/nrf52840.repl" --max-steps 45000000 "$artifacts/q15.elf"
    grep -Fq 'PROJECT EXECUTION SUCCESSFUL' "$artifacts/q15.log"
)

check_i2c() (
    request=$1
    run "$request" --platform "$artifacts/bme.repl" --max-steps 35000000 \
        --uart-event "10000000:$artifacts/import.in" --uart-event "12000000:$artifacts/init.in" \
        --uart-event "14000000:$artifacts/$request.in" "$artifacts/micropython.elf"
    awk '/^>>> import machine/ { imported = NR }
        /^>>> i = machine.I2C/ { opened = NR }
        /^>>> print/ { requested = NR }
        END { exit !(imported && imported < opened && opened < requested) }' "$artifacts/$request.log"
    if [ "$request" = chip ]; then
        grep -q '^ID: 0x60' "$artifacts/chip.log"
        ! grep -Eq 'Trying to write without specifying address|Unhandled read from offset 0x0|Unhandled write to offset 0x1C' "$artifacts/chip.log"
    else
        grep -Eq '^S: \[118\][[:space:]]*$' "$artifacts/scan.log"
    fi
)

failed=0
check_alarm & alarm=$!
check_button & button=$!
check_q15 & q15=$!
check_i2c chip & chip=$!
check_i2c scan & scan=$!
for case_name in alarm button q15 chip scan; do
    case $case_name in
        alarm) pid=$alarm ;;
        button) pid=$button ;;
        q15) pid=$q15 ;;
        chip) pid=$chip ;;
        scan) pid=$scan ;;
    esac
    if wait "$pid"; then
        printf 'PASS %s %s\n' "$case_name" "$artifacts/$case_name.log"
    else
        printf 'FAIL %s %s\n' "$case_name" "$artifacts/$case_name.log" >&2
        failed=1
    fi
done
test "$failed" -eq 0
