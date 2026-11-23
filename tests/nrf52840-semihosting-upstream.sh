#!/bin/sh
# Fixed Cortex-M console and exit cases from arm-semihosting.robot.
set -eu
if [ "$#" -ne 4 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR ELF_CACHE" >&2
    exit 1
fi
penv=$1
robot=$2/tests/unit-tests/arm-semihosting.robot
out=$3
cache=$4
mkdir -p "$out" "$cache"
cat >"$out/console.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
rom: Memory.MappedMemory @ sysbus 0 { size: 0x01010000 }
sram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x1000 }
ram: Memory.MappedMemory @ sysbus 0x60000000 { size: 0x01000000 }
semihosting: CPU.SemihostingHandler @ cpu
console: UART.SemihostingUart @ semihosting .InputOutputError
REPL
sed 's/\.InputOutputError$/.InputOutput/' "$out/console.repl" >"$out/split.repl"
echo 'errorConsole: UART.SemihostingUart @ semihosting .Error' >>"$out/split.repl"

run_case() {
    key=$1
    label=$2
    platform=$3
    reason=$4
    expected_code=$5
    file=$(awk -v key="\${$key}" '$1 == key {print $2}' "$robot")
    file=${file#*/}
    if [ ! -f "$cache/$file" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$file" -o "$cache/$file.part"
        mv "$cache/$file.part" "$cache/$file"
    fi
    printf '%s  %s\n' "${file##*-}" "$cache/$file" | sha1sum --check
    "$penv" run --platform "$out/$platform.repl" --max-steps 100000 "$cache/$file" \
        >"$out/$label.stdout" 2>"$out/$label.stderr"
    cat "$out/$label.stdout" "$out/$label.stderr"
    line="cpu.semihosting: Program exited with reason $reason"
    if [ "$expected_code" != none ]; then
        line="$line and return code $expected_code"
    fi
    grep -Fxq "$line" "$out/$label.stderr"
}
application='ADP_Stopped_ApplicationExit(0x20026)'
run_case EXIT exit console "$application" none
run_case EXIT-FAILURE exit-failure console 'ADP_Stopped_RunTimeErrorUnknown(0x20023)' none
run_case EXIT-EXTENDED exit-extended console "$application" 0
run_case EXIT-EXTENDED-FAILURE exit-extended-failure console "$application" 1
for operation in WRITE0 WRITEC WRITE-TTY; do
    run_case "$operation" "$operation" console "$application" 0
    grep -Fxq SUCCESS "$out/$operation.stdout"
done
run_case WRITE-TTY write-tty-split split "$application" 0
grep -Fxq SUCCESS "$out/write-tty-split.stdout"
grep -Fxq ERROR "$out/write-tty-split.stderr"
if grep -Fxq ERROR "$out/write-tty-split.stdout" || grep -Fxq SUCCESS "$out/write-tty-split.stderr"; then
    exit 1
fi

# Check Simple Semihosting Call: preserve the two argument blocks and
# original instructions. Probe R0 after the second BKPT before the loop.
cat >"$out/simple.s" <<'ASM'
.syntax unified
.thumb
.global start
.section .vectors, "a"
.word 0x20001000, start
.text
.thumb_func
start:
    ldr r0, =0x01
    ldr r1, =0x20000000
    bkpt 0xab
    ldr r0, =0x05
    ldr r1, =0x20000100
    bkpt 0xab
    nop
    cmp r0, #0
    bne 1f
    bkpt 0
1:  b .
.section .arguments, "a"
.word path, 4, 3
path: .asciz ":tt"
.org 0x100
.word 1, message, 8
message: .asciz "SUCCESS\n"
ASM
cat >"$out/simple.ld" <<'LD'
PHDRS {
    code PT_LOAD FLAGS(5);
    args PT_LOAD FLAGS(4);
}
SECTIONS {
    .vectors 0 : { *(.vectors) } :code
    .text 0x100 : { *(.text) } :code
    .arguments 0x20000000 : { *(.arguments) } :args
    /DISCARD/ : { *(.ARM.attributes) }
}
LD
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/simple.s" -o "$out/simple.o"
ld.lld -T "$out/simple.ld" --entry=start "$out/simple.o" -o "$out/simple.elf"
"$penv" run --platform "$out/console.repl" --max-steps 100 "$out/simple.elf" \
    >"$out/simple.stdout" 2>"$out/simple.stderr"
cat "$out/simple.stdout" "$out/simple.stderr"
grep -Fxq SUCCESS "$out/simple.stdout"
