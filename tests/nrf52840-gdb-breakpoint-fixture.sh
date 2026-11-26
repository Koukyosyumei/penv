#!/bin/sh
# Guest fixture for gdb.robot:410-457. The monitor check only covers the first stop.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$2
out=$3
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0x100 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
flash: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x40000 }
REPL
cat > "$out/guest.s" <<'ASM'
.syntax unified
.thumb
.global reset
.thumb_func
reset:
    // A 32-bit register instruction at each address, including 0x10, 0x20, 0x30, 0x40.
    .rept 17
    movw r0, #0
    .endr
    b .
.org 0x100
.word 0x20040000, reset
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/guest.s" -o "$out/guest.o"
ld.lld -N -Ttext=0 --entry=reset "$out/guest.o" -o "$out/guest.elf"
cat > "$out/base.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @guest.elf
cpu MaximumBlockSize 1
RESC
cp "$out/base.resc" "$out/debug.resc"
echo 'machine StartGdbServer 3333' >> "$out/debug.resc"
cat > "$out/stop.gdb" <<'GDB'
target remote :3333
break *0x10
continue
GDB
cp "$out/stop.gdb" "$out/step.gdb"
echo stepi >> "$out/step.gdb"
cat > "$out/multiple.gdb" <<'GDB'
target remote :3333
break *0x10
break *0x20
break *0x30
break *0x40
continue
continue
continue
continue
GDB
for address in 10 20 30 40; do
    file="$out/stop-$address.resc"
    cp "$out/base.resc" "$file"
    printf '%s\n' "cpu AddPauseHook 0x$address" >> "$file"
    status=0
    "$penv" run --script "$file" --platform-root "$root" --max-steps 100 \
        > "$out/stop-$address.log" 2>&1 || status=$?
    printf '%s\n' "$status" > "$out/stop-$address.status"
    [ "$status" -eq 0 ] || { cat "$out/stop-$address.log" >&2; exit 1; }
    grep -Fxq "penv: machine paused at pc=0x000000$address" "$out/stop-$address.log"
    printf 'monitor stop at 0x%s: PASS\n' "$address"
done

cp "$out/base.resc" "$out/reset.resc"
cat >> "$out/reset.resc" <<'RESC'
cpu AddLogHook 0x10 "reached 0x10"
cpu AddPauseHook 0x10
cpu Step 100
machine Reset
cpu Step 100
RESC
status=0
"$penv" run --script "$out/reset.resc" --platform-root "$root" --max-steps 100 \
    > "$out/reset.log" 2>&1 || status=$?
printf '%s\n' "$status" > "$out/reset.status"
[ "$status" -eq 0 ]
[ "$(grep -Fxc 'reached 0x10' "$out/reset.log")" -eq 2 ]
grep -Fxq 'penv: machine paused at pc=0x00000010' "$out/reset.log"
echo 'monitor address hook after machine reset: PASS'

cp "$out/base.resc" "$out/step-monitor.resc"
echo 'cpu Step 5' >> "$out/step-monitor.resc"
status=0
"$penv" run --script "$out/step-monitor.resc" --platform-root "$root" --max-steps 100 \
    > "$out/step-monitor.log" 2>&1 || status=$?
printf '%s\n' "$status" > "$out/step-monitor.status"
[ "$status" -eq 0 ]
grep -Fxq 'penv: machine paused at pc=0x00000014' "$out/step-monitor.log"
echo '32-bit instruction length at 0x10: PASS'
