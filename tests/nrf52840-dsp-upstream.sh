#!/bin/sh
# Shared Cortex-M4F CMSIS-DSP Robot suite, using its CPU, memory and UART fixture.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
robot=$2/tests/unit-tests/arm-dsp-examples-cortex-m4.robot
out=$3
mkdir -p "$out"
cat >"$out/cortex-m4f.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4f"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
rom: Memory.MappedMemory @ sysbus 0
    size: 0x40000
ram: Memory.MappedMemory @ sysbus 0x20000000
    size: 0x100000
serial: UART.TrivialUart @ sysbus 0x40000000
REPL

check() {
    example=$1
    shift
    remote=$(awk -v pattern="/arm_${example}_example-s_" '
        /sysbus LoadELF/ && index($NF, pattern) { sub(/.*\//, "", $NF); print $NF }
    ' "$robot")
    elf=$out/arm_${example}_example.elf
    log=$out/$example.log
    if [ ! -f "$elf" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$elf.part"
        mv "$elf.part" "$elf"
    fi
    printf '%s  %s\n' "${remote##*-}" "$elf" | sha1sum --check --status
    status=0
    "$penv" run --platform "$out/cortex-m4f.repl" --max-steps 5000000 "$elf" >"$log" 2>&1 || status=$?
    if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
        cat "$log" >&2
        return "$status"
    fi
    # Match all upstream UART expectations in their declared order.
    patterns=$(printf '%s\n' "$@")
    ascii=$out/$example.txt
    tr -d '\000' <"$log" >"$ascii"
    awk -v patterns="$patterns" '
        BEGIN { count = split(patterns, expected, "\n"); next_line = 1 }
        next_line <= count && $0 ~ expected[next_line] { next_line++ }
        END { exit !(next_line > count) }
    ' "$ascii"
    printf 'PASS %s %s\n' "$example" "$log"
}

check bayes 'Class = 0' 'Class = 1' 'Class = 2'
check class_marks 'mean = 212[.]3[0-9]*, std = 50[.]9'
for example in convolution dotproduct fft_bin fir graphic_equalizer linear_interp matrix signal_convergence sin_cos; do
    check "$example" SUCCESS
done
check svm 'Result = 0' 'Result = 1'
check variance SUCCESS
