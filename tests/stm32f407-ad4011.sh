#!/usr/bin/env bash
# Exercise the upstream direct voltage sweep through STM32F4 SPI MMIO.
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-ad4011}
renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"
cat > "$work/adc.ld" <<'LD'
ENTRY(reset)
SECTIONS {
    .vectors 0x08000000 : { *(.vectors) }
    .text : { *(.text) }
}
LD
cat > "$work/adc.S" <<'ASM'
.syntax unified
.thumb
.section .vectors,"a"
.word 0x20020000, reset
.text
.global reset
.thumb_func
reset:
    ldr r0, =0x40023800
    ldr r1, [r0, #0x40]
    orr r1, r1, #0x20000
    str r1, [r0, #0x40]
    ldr r1, [r0, #0x44]
    orr r1, r1, #0x1000
    str r1, [r0, #0x44]
    ldr r4, =0x40004400
    movw r1, #0x2008
    str r1, [r4, #12]
    ldr r0, =0x40013000
    movs r1, #0x44
    str r1, [r0]
    movs r2, #3
    movs r3, #0
next:
    strb r3, [r0, #12]
    ldrb r1, [r0, #12]
    str r1, [r4, #4]
    subs r2, #1
    bne next
    bkpt #0
ASM
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
    -Wl,-T,"$work/adc.ld" "$work/adc.S" -o "$work/adc.elf"

failed=0
for ((input=0; input<4820000; input+=500000)); do
    cat > "$work/adc-$input.repl" <<REPL
using "platforms/cpus/stm32f4.repl"
adc: Analog.AD4011_ADC @ spi1 { referenceVoltage: 4.82 }
vin: Analog.ADCChannelSource @ adc 0
    defaultValueMicroVolts: $input
REPL
    cat > "$work/adc-$input.resc" <<RESC
mach create
machine LoadPlatformDescription @adc-$input.repl
sysbus LoadELF @adc.elf
showAnalyzer sysbus.usart2
RESC
    if "$penv" run --script "$work/adc-$input.resc" --platform-root "$renode" --max-steps 1000 \
        > "$work/adc-$input.uart" 2> "$work/adc-$input.log" &&
        python3 - "$input" "$work/adc-$input.uart" <<'PY'
from pathlib import Path
import sys

first, second, third = Path(sys.argv[2]).read_bytes()
raw = (first << 10) | (second << 2) | (third & 3)
voltage = raw * 4.82 / (1 << 17)
expected = int(sys.argv[1]) / 1e6
assert abs(voltage - expected) < 0.00005, (expected, voltage)
print(f"PASS\t{expected:.2f} V\t{voltage:.8f} V")
PY
    then
        :
    else
        printf 'FAIL\t%s microvolts\n' "$input"
        failed=1
    fi
done
exit "$failed"
