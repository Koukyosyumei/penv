#!/bin/sh
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1001000 }
REPL
cat > "$out/loop.s" <<'ASM'
.syntax unified
.thumb
.word 0x2000, loop
.org 0x1000
.global loop
.thumb_func
loop:
    nop
    nop
    nop
    b loop
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/loop.s" -o "$out/loop.o"
ld.lld -N -Ttext=0 --entry=loop "$out/loop.o" -o "$out/loop.elf"
for disabled in false true; do
    cat > "$out/$disabled.resc" <<RESC
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @loop.elf
cpu MaximumBlockSize 1
cpu SetHookAtBlockBegin "self.DebugLog('block started: ' + 'PC '+ str(self.PC))"
cpu PC 0x1000
cpu SyncPCEveryInstructionDisabled $disabled
RESC
    status=0
    # Upstream runs 20 us at 1 MIPS, or 20 instruction slots.
    "$penv" run --script "$out/$disabled.resc" --max-steps 20 > "$out/$disabled.log" 2> "$out/$disabled.stderr" || status=$?
    printf '%s\n' "$status" > "$out/$disabled.status"
    if [ "$disabled" = true ]; then
        [ "$status" -eq 1 ]
        grep -Fq 'disabling Cortex-M PC synchronization requires translation block chaining' "$out/$disabled.stderr"
        echo 'sync_pc.robot:157-166 BLOCKED (translation block chaining)'
    else
        [ "$status" -eq 2 ] || { cat "$out/$disabled.stderr" >&2; exit 1; }
        [ "$(grep -Fxc 'block started: PC 0x1004' "$out/$disabled.log")" -ge 2 ]
        [ "$(grep -Fc 'block started: PC ' "$out/$disabled.log")" -eq 20 ]
        echo 'sync_pc.robot:167-178 PASS (Cortex-M4, PC synchronization enabled)'
    fi
done
