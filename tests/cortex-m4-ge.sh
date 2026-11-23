#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
renode=${RENODE_ROOT:-../renode}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"
cargo build --release
cat > "$work/board.repl" <<'REPL'
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x8000000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x10000
uart: UART.LiteX_UART @ sysbus 0x40000000
REPL
cat > "$work/ge.ld" <<'LD'
ENTRY(reset)
SECTIONS {
    .text 0 : { *(.text) }
    .vectors 0x10000 : { *(.vectors) }
}
LD
python3 - "$renode/tests/unit-tests/arm-ge-flag.robot" "$work" <<'PY'
import re
import sys
from pathlib import Path

source, directory = map(Path, sys.argv[1:])
suite = source.read_text().split('*** Test Cases ***', 1)[1]
def emit(name, program, expected):
    (directory / f'{name}.expected').write_bytes(expected.to_bytes(4, 'little'))
    (directory / f'{name}.s').write_text(
        '.syntax unified\n.thumb\n.text\n.global reset\n.thumb_func\nreset:\n'
        + '\n'.join(program)
        + '\nldr r5, =0x40000000\n'
        + '\nlsrs r4,r4,#8\n'.join(['strb r4,[r5]'] * 4)
        + '\nbkpt #0\n'
        + '.section .vectors,"a"\n.word 0x8000000, reset\n'
    )

for name in ('sadd8', 'sadd16', 'ssub8', 'ssub16', 'usub8', 'usub16'):
    case = re.search(rf'^\S[^\n]*\({name}\)\n(.*?)(?=^\S|\Z)', suite, re.M | re.S)[1]
    program = re.findall(r'^ +\.\.\. +(.+)$', case, re.M)
    assert len(program) == 10 and program[-1] == 'sel r4, r1, r0'
    assert re.search(r'cpu Step 10\s', case) and 'GetRegister "R4"' in case
    expected = int(re.search(r'Should Be Equal As Numbers\s+\$\{ret\}\s+(0x[0-9A-F]+)', case)[1], 16)
    emit(name, program, expected)

# The upstream row named usub8 actually executes ssub8. Exercise USUB8
# separately with high-bit operands that distinguish the two instructions.
emit('usub8-native', [
    'movs r0, #0', 'mvn r1, r0',
    'movw r2, #0x00ff', 'movt r2, #0x8000',
    'movw r3, #0x0101', 'movt r3, #0x0101',
    'usub8 r2, r2, r3', 'sel r4, r1, r0',
], 0xff0000ff)
PY
for name in sadd8 sadd16 ssub8 ssub16 usub8 usub16 usub8-native; do
    "${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb \
        -nostdlib -fuse-ld=lld -Wl,-T,"$work/ge.ld" "$work/$name.s" -o "$work/$name.elf"
    target/release/penv run --platform "$work/board.repl" --max-steps 32 "$work/$name.elf" > "$work/$name.uart"
    cmp "$work/$name.expected" "$work/$name.uart"
    python3 -c 'import sys; from pathlib import Path; print(sys.argv[1], "R4=0x{:08x}".format(int.from_bytes(Path(sys.argv[2]).read_bytes(), "little")))' "$name" "$work/$name.uart"
done
