#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-timed-tests}
renode=$(realpath "${RENODE_ROOT:-../renode}")
mkdir -p "$work"
cargo build --release

cat > "$work/rtc-32khz.repl" <<'REPL'
using "platforms/cpus/stm32f4.repl"

rtc:
    wakeupTimerFrequency: 32000
REPL

run() {
    local name=$1 platform=$2 sha=$3 asset=$4
    local elf="$work/$asset-$sha"
    cat > "$work/$name.tsv"
    if [[ ! -f "$elf" ]]; then
        curl --fail --location "https://dl.antmicro.com/projects/renode/$asset-$sha" --output "$elf.part"
        mv "$elf.part" "$elf"
    fi
    printf '%s  %s\n' "$sha" "$elf" | sha1sum --check -
    cat > "$work/$name.resc" <<RESC
mach create
machine LoadPlatformDescription @$(realpath "$platform")
sysbus LoadELF @$(basename "$elf")
showAnalyzer sysbus.usart2
RESC
    target/release/penv run --script "$work/$name.resc" --platform-root "$renode" \
        --virtual-mips 100 --f4-repl-timing --uart-check "$work/$name.tsv" \
        >"$work/$name.log" 2>"$work/$name-check.log" || {
        cat "$work/$name-check.log" "$work/$name.log" >&2
        return 1
    }
    cat "$work/$name-check.log"
}

case ${2:-all} in
all|rtc)
    run rtc "$work/rtc-32khz.repl" ca2effb6a0a8bcde39496b99bcb8b160a4ed292e \
        stm32f4_discovery--riot-tests_periph_rtc.elf-s_1249644 <<'UART'
wait	8	Help: Press s to start test, r to print it is ready
send	s
wait	8	This is RIOT!
wait	8	RIOT RTC low-level driver test
wait	8	This test will display 'Alarm!' every 2 seconds for 4 times
wait	8	Alarm!
second	2
wait	8	Alarm!
second	4
wait	8	Alarm!
second	6
wait	8	Alarm!
second	8
idle	3
UART
    ;;
esac

case ${2:-all} in
all|faultmask)
    run faultmask "$renode/platforms/cpus/stm32f4.repl" 080256edf201b1e2f7c67bf15000ba1ffa031990 \
        stm32f4disco-faultmask.elf-s_434744 <<'UART'
wait	8	Setting FAULTMASK to 1
wait	8	Timer IRQ enabled
wait	8	Sleeping
idle	1
wait	8	Setting FAULTMASK to 0
wait	8	Timer interrupt
wait	8	Timer interrupt
wait	8	Timer interrupt
UART
    run faultmask-exit "$renode/platforms/cpus/stm32f4.repl" 802f85357028150b4586bb3a54a5e44e7a3c2ec5 \
        stm32f4disco-faultmask-noclearing.elf-s_433676 <<'UART'
wait	8	Timer interrupt
wait	8	Timer interrupt
wait	8	Timer interrupt
wait	8	Timer interrupt
UART
    ;;
esac
