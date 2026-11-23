#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
penv=${PENV_BIN:-target/release/penv}
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
mkdir -p "$assets"
base=${1:?Pass the directory printed by tests/stm32h7-zephyr-smoke.sh}
work=$(mktemp -d)
printf 'STM32H7 sensor and Ethernet logs: %s\n' "$work"

# Apply two signed vectors from IIS2MDC.robot and both VEML7700 shell levels
# using the reference shell ELF and static REPL input for each run.
mkdir "$work/positive" "$work/negative"
for case in positive negative; do
    if [[ $case == positive ]]; then
        field='15000 30000 150'
        lux=500
        expected_light=499.000000
        expected_magnet='0.150000 0.300000 0.001500'
    else
        field='-123450 67800 0'
        lux=1000
        expected_light=999.000000
        expected_magnet='-1.234500 0.678000 0.000000'
    fi
    read -r x y z <<< "$field"
    awk -v lux="$lux" -v x="$x" -v y="$y" -v z="$z" '
        /^    Illuminance:/ {$2 = lux; print "    Illuminance: " $2; next}
        {print}
        /^iis2mdc:/ {
            print "    MagneticFluxDensityX: " x
            print "    MagneticFluxDensityY: " y
            print "    MagneticFluxDensityZ: " z
        }
    ' "$base/boot.repl" > "$work/$case/boot.repl"
    cp "$base/boot.resc" "$work/$case/boot.resc"
    status=0
    "$penv" run --script "$work/$case/boot.resc" \
        --platform-root "$renode" --asset-dir "$assets" --max-steps 2600000 \
        --uart-event "1200000:$base/sensor.txt" --uart-event "1700000:$base/magnetometer.txt" \
        > "$work/$case/uart.log" 2> "$work/$case/penv.log" || status=$?
    if [[ $status != 2 ]] ||
        ! rg -aF 'channel type=18(light)' "$work/$case/uart.log" | rg -qF "($expected_light)" ||
        ! awk -v expected="$expected_magnet" '
            BEGIN { split(expected, want, " ") }
            /channel type=11\(magn_xyz\)/ {
                sample = $0
                sub(/^.*ns, \(/, "", sample)
                sub(/\).*/, "", sample)
                if (split(sample, got, ", ") != 3) next
                for (i = 1; i <= 3; i++) {
                    delta = got[i] - want[i]
                    if (delta < -0.000002 || delta > 0.000002) next
                }
                matched = 1
            }
            END { exit !matched }
        ' "$work/$case/uart.log"; then
        cat "$work/$case/penv.log" "$work/$case/uart.log" >&2
        exit 1
    fi
done

# Use the two ELF images from nucleo_h753zi.robot on a virtual switch.
cp "$base/boot.repl" "$work/echo.repl"
cat >> "$work/echo.repl" <<'REPL'

usart3: UART.STM32F7_USART @ sysbus 0x40004800
    frequency: 125000000
    IRQ -> nvic@39
REPL
sed -i '/BasicStatus:/a\    VendorSpecific15: 0x1058' "$work/echo.repl"

robot="$renode/tests/platforms/nucleo_h753zi.robot"
{
    echo 'emulation CreateSwitch "lan"'
    for node in server client; do
        tag=${node^^}
        name=$(awk -v tag="\${ECHO_$tag}" '$1 == tag {sub(/^.*\//,"",$2); print $2}' "$robot")
        [[ -n $name ]]
        url="https://dl.antmicro.com/projects/renode/$name"
        if [[ ! -f "$assets/$name" ]]; then
            curl --fail --location "$url" --output "$assets/$name.part"
            mv "$assets/$name.part" "$assets/$name"
        fi
        printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check - >&2
        cat <<RESC
mach create "$node"
machine LoadPlatformDescription @echo.repl
sysbus LoadELF @$url
cpu VectorTableOffset \`sysbus GetSymbolAddress "_vector_table"\`
connector Connect sysbus.ethernet lan
RESC
    done
    cat <<'RESC'
mach set "client"
sysbus ReadDoubleWord 0x24000924
emulation RunFor "10"
sysbus ReadDoubleWord 0x24000924
RESC
} > "$work/echo.resc"
mkdir "$work/uart"
status=0
"$penv" run --script "$work/echo.resc" --platform-root "$renode" \
    --asset-dir "$assets" --max-steps 10000000 --uart-log-dir "$work/uart" \
    > "$work/echo.log" 2> "$work/echo-penv.log" || status=$?
server="$work/uart/0.uart.log"
client="$work/uart/1.uart.log"
if [[ $status != 0 ]] ||
    ! awk '/client: sysbus\[0x24000924\]/ { print $NF }' "$work/echo.log" | { read -r before && read -r after && [[ $before == 0x00000000 ]] && (( after >= 4 )); } ||
    ! rg -aqF 'net_echo_client_sample: Network connected' "$client" ||
    ! rg -aqF 'Accepted connection' "$server" ||
    [[ $(rg -ac 'TCP .*Received and replied' "$server") -lt 4 ]]; then
    cat "$work/echo-penv.log" "$work/echo.log" "$server" "$client" >&2
    exit 1
fi

# Exercise the older Zephyr I2C driver's TXDR-before-START path with the Robot ELF.
poll_url=$(sed -n 's/^${POLLING_BIN}[[:space:]]*//p' "$renode/tests/peripherals/IIS2MDC.robot")
poll_elf="$assets/${poll_url##*/}"
if [[ ! -f "$poll_elf" ]]; then
    curl --fail --location "$poll_url" --output "$poll_elf.part"
    mv "$poll_elf.part" "$poll_elf"
fi
printf '%s  %s\n' 5599c7a86f3d898bc083c2a30d12ede98f9d2fec "$poll_elf" | sha1sum --check -
for case in positive negative; do
    cp "$work/$case/boot.repl" "$work/$case/poll.repl"
    cat >> "$work/$case/poll.repl" <<'REPL'

usart3: UART.STM32F7_USART @ sysbus 0x40004800
    frequency: 125000000
    IRQ -> nvic@39

lpuart1: UART.STM32F7_USART @ sysbus 0x58000c00
    frequency: 125000000
    IRQ -> exti@64
REPL
    sed 's/@boot.repl/@poll.repl/' "$work/$case/boot.resc" > "$work/$case/poll.resc"
    status=0
    "$penv" run --script "$work/$case/poll.resc" \
        --platform-root "$renode" --asset-dir "$assets" --max-steps 2500000 "$poll_elf" \
        > "$work/$case/poll-uart.log" 2> "$work/$case/poll-penv.log" || status=$?
    if [[ $case == positive ]]; then
        expected='0\.150000 +0\.300000 +0\.001500'
    else
        expected='-1\.234500 +0\.678000 +0\.000000'
    fi
    if [[ $status != 2 ]] || ! rg -aq "\\( x y z \\) = \\( +$expected \\)" "$work/$case/poll-uart.log"; then
        cat "$work/$case/poll-penv.log" "$work/$case/poll-uart.log" >&2
        exit 1
    fi
done

# Replay the upstream sample fixture after its original 3-second offset.
mkdir "$work/timed"
samples="$renode/tests/peripherals/iis2mdc-samples.csv"
awk -F, 'BEGIN { OFS = "," } NR > 1 { $1 = sprintf("%.0f", $1 + 3000000000) } { print }' \
    "$samples" > "$work/timed/samples.csv"
awk -v csv="$work/timed/samples.csv" '
    /^    MagneticFluxDensity[XYZ]:/ { next }
    { print }
    /^iis2mdc:/ {
        print "    SampleCsv: \"" csv "\""
        print "    -> gpioPortG@11"
    }
' "$work/positive/poll.repl" > "$work/timed/poll.repl"
cp "$work/positive/poll.resc" "$work/timed/poll.resc"
trigger_url=$(sed -n 's/^${TRIGGER_BIN}[[:space:]]*//p' "$renode/tests/peripherals/IIS2MDC.robot")
trigger_elf="$assets/${trigger_url##*/}"
if [[ ! -f "$trigger_elf" ]]; then
    curl --fail --location "$trigger_url" --output "$trigger_elf.part"
    mv "$trigger_elf.part" "$trigger_elf"
fi
printf '%s  %s\n' 26f5a81694b9ea2b6c35a05a6c901d30126c303d "$trigger_elf" | sha1sum --check -
awk -F, 'NR > 1 { printf "%.6f %.6f %.6f\n", $2/100000, $3/100000, $4/100000 }' \
    "$samples" | sed "s/-0\\.000000/0.000000/g" > "$work/timed/expected"
for mode in poll trigger; do
    elf="$poll_elf"
    if [[ $mode == trigger ]]; then elf="$trigger_elf"; fi
    status=0
    "$penv" run --script "$work/timed/poll.resc" \
        --platform-root "$renode" --asset-dir "$assets" --max-steps 11000000 "$elf" \
        > "$work/timed/$mode.uart" 2> "$work/timed/$mode.err" || status=$?
    if [[ $mode == trigger ]]; then
        sed -nE 's/^ *iis2mdc@1e \(x, y, z\): *\( *([^,]+), *([^,]+), *([^)]*)\).*/\1 \2 \3/p' \
            "$work/timed/$mode.uart" > "$work/timed/$mode.actual"
    else
        sed -nE 's/^\( x y z \) = \( *([^ ]+) +([^ ]+) +([^ ]+) \).*/\1 \2 \3/p' \
            "$work/timed/$mode.uart" | awk '$0 != previous && $0 != "0.000000 0.000000 0.000000" { print; previous = $0 }' \
            > "$work/timed/$mode.actual"
    fi
    if [[ $status != 2 ]] || ! diff -u "$work/timed/expected" "$work/timed/$mode.actual"; then
        cat "$work/timed/$mode.err" "$work/timed/$mode.uart" >&2
        exit 1
    fi
done

# Sample the TIM3_CH1 waveform at both edges over GPIOB4 AF2.
mkdir "$work/pwm"
cp "$base"/boot.{resc,repl} "$work/pwm"
cat >> "$work/pwm/boot.resc" <<'RESC'
emulation RunFor "2"
sysbus WriteDoubleWord 0x580244e0 0x2
sysbus WriteDoubleWord 0x58020400 0x200
sysbus WriteDoubleWord 0x58020420 0x20000
sysbus WriteDoubleWord 0x580244e8 2
sysbus WriteDoubleWord 0x40000400 0
sysbus WriteDoubleWord 0x40000428 24999
sysbus WriteDoubleWord 0x4000042c 99
sysbus WriteDoubleWord 0x40000434 25
sysbus WriteDoubleWord 0x40000418 0x60
sysbus WriteDoubleWord 0x40000420 1
sysbus WriteDoubleWord 0x40000414 1
sysbus WriteDoubleWord 0x40000400 1
emulation RunFor "0.0024"
sysbus ReadDoubleWord 0x58020410
emulation RunFor "0.0002"
sysbus ReadDoubleWord 0x58020410
emulation RunFor "0.0073"
sysbus ReadDoubleWord 0x58020410
emulation RunFor "0.0002"
sysbus ReadDoubleWord 0x58020410
RESC
"$penv" run --script "$work/pwm/boot.resc" --platform-root "$renode" \
    --asset-dir "$assets" --max-steps 3000000 --uart-log-dir "$work/pwm" \
    > "$work/pwm/waveform.log" 2> "$work/pwm/penv.log"
rg -aqF 'Booting STM32H7 Renode Platform Demo' "$work/pwm/0.uart.log"
printf '%s\n' 0x00000010 0x00000000 0x00000000 0x00000010 |
    diff -u - <(awk '/sysbus\[0x58020410\]/ { print $NF }' "$work/pwm/waveform.log")
