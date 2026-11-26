#!/bin/sh
# execution-tracing.robot:600-626,854-859
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x100
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x1000
REPL
cat >"$out/trace.s" <<'ASM'
.syntax unified
.thumb
.global reset
.thumb_func
reset:
    .hword 0x2010, 0xf380, 0x8811, 0x4806, 0xf44f, 0x6100
.org 0x100
.word 0x1000, reset
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/trace.s" -o "$out/trace.o"
ld.lld -N -Ttext=0 --entry=reset "$out/trace.o" -o "$out/trace.elf"

for format in text binary error; do
    case "$format" in
        text) mode='PCAndOpcode false'; expected_status=2 ;;
        binary) mode='PCAndOpcode true'; expected_status=2 ;;
        error) mode='Disassembly true'; expected_status=1 ;;
    esac
    cat >"$out/$format.resc" <<RESC
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @trace.elf
cpu CreateExecutionTracing "tracer" @$format.trace $mode
RESC
    status=0
    "$penv" run --script "$out/$format.resc" --max-steps 4 \
        >"$out/$format.stdout" 2>"$out/$format.stderr" || status=$?
    [ "$status" -eq "$expected_status" ]
done
cat >"$out/text.expected" <<'TRACE'
0x0: 0x2010
0x2: 0xF3808811
0x6: 0x4806
0x8: 0xF44F6100
TRACE
cmp "$out/text.expected" "$out/text.trace"
printf 'ReTrace\005\004\001\001\017thumb cortex-m4\000\000\000\000\002\020\040\000\002\000\000\000\004\200\363\021\210\000\006\000\000\000\002\006\110\000\010\000\000\000\004\117\364\000\141\000' \
    >"$out/binary.expected"
cmp "$out/binary.expected" "$out/binary.trace"
[ "$(wc -c <"$out/binary.trace")" -eq 63 ]
grep -Fq "don't support binary output file with the Disassembly formatting" "$out/error.stderr"
[ ! -e "$out/error.trace" ]
cat "$out/text.trace" "$out/error.stderr"
