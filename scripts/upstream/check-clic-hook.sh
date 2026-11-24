#!/usr/bin/env bash
set -eu
binary=$(realpath "$1")
root=$(realpath "$2")
out=$(realpath -m "$3")
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32im"
    timeProvider: empty
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x40000
REPL
cat >"$out/board.ld" <<'LD'
ENTRY(_start)
PHDRS { guest PT_LOAD FLAGS(7); }
SECTIONS {
  . = 0x80000000;
  .text : { *(.text) } :guest
  .data : { *(.data) } :guest
}
LD
cat >"$out/signature.s" <<'ASM'
.text
.globl _start
.globl rvtest_code_end
_start:
    wfi
rvtest_code_end:
    nop
.data
.globl begin_signature
.globl end_signature
begin_signature:
    .word 0x11223344
end_signature:
ASM
llvm-mc -triple=riscv32 -filetype=obj "$out/signature.s" -o "$out/signature.o"
ld.lld -m elf32lriscv -T "$out/board.ld" "$out/signature.o" -o "$out/signature.elf"
for scenario in match mismatch wait; do
    sig=11223344
    nop=true
    case "$scenario" in
        mismatch) sig=11223345 ;;
        wait) nop=false ;;
    esac
    printf '%s\n' "$sig" >"$out/$scenario.signature"
    {
        printf 'mach create\nmachine LoadPlatformDescription @"%s/board.repl"\n' "$out"
        printf 'machine UserState @"%s/%s.signature"\n' "$out" "$scenario"
        sed -n '/^set end_hookscript$/,/^set sigwrite_hookscript$/{ /^set sigwrite_hookscript$/d; p; }' \
            "$root/tests/peripherals/CLIC/CLIC-test-setup.resc"
        printf 'sysbus LoadELF @"%s/signature.elf"\n' "$out"
        printf '%s\n' 'cpu AddHook `sysbus GetSymbolAddress "rvtest_code_end"` $end_hookscript'
        printf 'sysbus LogPeripheralAccess ram\ncpu WfiAsNop %s\n' "$nop"
    } >"$out/$scenario.resc"
    status=0
    "$binary" run --script "$out/$scenario.resc" --platform-root "$root" --max-steps 4 \
        >"$out/$scenario.stdout" 2>"$out/$scenario.stderr" || status=$?
    test "$status" -eq 2
done
grep -F 'All signatures correct' "$out/match.stderr"
grep -F 'ram: ReadByte' "$out/match.stderr"
grep -F 'Wrong generated signature #0: 11223344 different than reference signature 11223345' "$out/mismatch.stderr"
! grep -F 'All signatures correct' "$out/mismatch.stderr" || exit 1
! grep -F 'Sig path:' "$out/wait.stderr" || exit 1
grep -i 'limit' "$out/wait.stderr"
