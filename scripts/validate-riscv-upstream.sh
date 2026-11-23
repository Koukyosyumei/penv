#!/usr/bin/env bash
# Run against a Renode checkout with the original scripts and platform definitions.
set -euo pipefail

if (( $# != 3 )); then
    echo "Usage: bash $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
renode_root=$2
artifacts=$3
assets=$artifacts/assets
mkdir -p "$assets"

nexys_video=$renode_root/scripts/single-node/litex_nexys_video_vexriscv_linux.resc
arty=$renode_root/scripts/single-node/arty_litex_vexriscv.resc
murax=$renode_root/scripts/single-node/murax.resc
minerva=$renode_root/scripts/single-node/litex_minerva.resc
litex=$renode_root/scripts/single-node/litex_vexriscv_smp.resc
linux_single=$renode_root/scripts/single-node/litex_vexriscv_linux.resc
ibex=$renode_root/scripts/single-node/litex_ibex.resc
tock=$renode_root/scripts/single-node/litex_vexriscv_tock.resc
zephyr=$renode_root/scripts/single-node/litex_vexriscv_zephyr.resc
i2s=scripts/complex/litex_i2s/litex_vexriscv_i2s.resc
git -C "$renode_root" rev-parse HEAD > "$artifacts/renode-revision"

prepare_assets() {
    local script=$1 url name size digest file
    local -a urls
    mapfile -t urls < <(awk '
        /^[[:space:]]*\$[a-zA-Z0-9_]+\??=@https:\/\// {
            split($1, assignment, "=")
            sub(/\?$/, "", assignment[1])
            variables[assignment[1]] = assignment[2]
        }
        $1 == "set" && $3 ~ /^@https:\/\// {
            variables["$" $2] = $3
        }
        ($1 == "sysbus" && ($2 == "LoadELF" || $2 == "LoadBinary" || $2 == "LoadFdt")) ||
        ($1 == "machine" && $2 == "SdCardFromFile") || $2 == "LoadPCM" || $2 == "ServeFile" {
            path = $3 in variables ? variables[$3] : $3
            if (path ~ /^@https:\/\//)
                print substr(path, 2)
        }
    ' "$script")
    if (( ${#urls[@]} == 0 )); then
        echo "No firmware URLs found in $script" >&2
        return 1
    fi
    for url in "${urls[@]}"; do
        name=${url##*/}
        if [[ ! $name =~ -s_([0-9]+)-([a-f0-9]{40})$ ]]; then
            echo "Expected a size and SHA-1 in $url" >&2
            return 1
        fi
        size=${BASH_REMATCH[1]}
        digest=${BASH_REMATCH[2]}
        file=$assets/$name
        if [[ ! -f $file ]]; then
            curl --fail --location --silent --show-error "$url" -o "$file.part"
            file=$file.part
        fi
        if [[ $(wc -c < "$file") != "$size" ]] ||
            ! printf '%s  %s\n' "$digest" "$file" | sha1sum --check --status; then
            echo "Firmware size or checksum mismatch for $file" >&2
            return 1
        fi
        if [[ $file == *.part ]]; then
            mv -- "$file" "$assets/$name"
        fi
    done
}

run_to_step_limit() {
    local name=$1 script=$2 status=0
    shift 2
    "$penv" run --script "$script" --platform-root "$renode_root" --asset-dir "$assets" \
        --stats "$@" > "$artifacts/$name.uart.log" 2> "$artifacts/$name.stderr.log" || status=$?
    printf '%s\n' "$status" > "$artifacts/$name.exit"
    if (( status != 2 )); then
        cat "$artifacts/$name.stderr.log" >&2
        echo "$name exited $status, expected step limit (2)" >&2
        return 1
    fi
}

prepare_assets "$murax"
printf nt > "$artifacts/murax.input"
run_to_step_limit murax "$murax" --uart-input "$artifacts/murax.input" \
    --uart-input-at 1000 --max-steps 100000
rg -aqF Ant "$artifacts/murax.uart.log"

prepare_assets "$ibex"
printf Q > "$artifacts/ibex-abort.input"
printf 'help\r' > "$artifacts/ibex-help.input"
run_to_step_limit litex-ibex "$ibex" --uart-event "35000000:$artifacts/ibex-abort.input" \
    --uart-event "36000000:$artifacts/ibex-help.input" --max-steps 38000000
for marker in 'BIOS CRC passed' 'Ibex @ 100MHz' 'Press Q or ESC to abort boot completely.' \
    'LiteX BIOS, available commands:'; do
    rg -aqF "$marker" "$artifacts/litex-ibex.uart.log"
done
rg -aq 'litex(\x1b\[[0-9;]*m)?>' "$artifacts/litex-ibex.uart.log"

prepare_assets "$zephyr"
printf 'version\r' > "$artifacts/zephyr-version.input"
run_to_step_limit litex-zephyr "$zephyr" --uart-event "4000000:$artifacts/zephyr-version.input" \
    --max-steps 12000000
rg -aqF 'uart:~$' "$artifacts/litex-zephyr.uart.log"
rg -aqF 'Zephyr version' "$artifacts/litex-zephyr.uart.log"

prepare_assets "$tock"
run_to_step_limit litex-tock "$tock" --max-steps 700000000
rg -aqF 'LiteX+VexRiscv on ArtyA7: initialization complete, entering main loop.' "$artifacts/litex-tock.uart.log"
# The upstream tester waits for the first D1 before checking three ordered pairs.
diff -u <(for _ in 1 2 3; do printf 'D1 says hello\nD2 says hello\n'; done) <(
    tr -d '\r' < "$artifacts/litex-tock.uart.log" |
        awk '/^D[12] says hello$/ && ($0 == "D1 says hello" || count) { print; if (++count == 6) exit }'
)

prepare_assets "$arty"
printf 'debug cas switches\r' > "$artifacts/arty-switches.input"
printf 'debug cas buttons read\r' > "$artifacts/arty-buttons.input"
printf 'debug cas buttons clear\r' > "$artifacts/arty-clear.input"
printf 'mdio_status\r' > "$artifacts/arty-mdio.input"
arty_events=(--litex-phy-event 0:eth.phy:0x0)
step=70000000
for part in switch button; do
    input=$artifacts/arty-switches.input
    if [[ $part == button ]]; then input=$artifacts/arty-buttons.input; fi
    arty_events+=(--uart-event "$step:$input")
    for pin in 0 1 2 3; do
        arty_events+=(--litex-button-event "$((step + 1000000)):cas.$part$pin:Toggle")
        step=$((step + 2000000))
        arty_events+=(--uart-event "$step:$input")
    done
    for pin in 0 1 2 3; do
        arty_events+=(--litex-button-event "$((step + 1000000)):cas.$part$pin:Toggle")
    done
    step=$((step + 2000000))
    arty_events+=(--uart-event "$step:$input")
    step=$((step + 2000000))
done
arty_events+=(--uart-event "$step:$artifacts/arty-clear.input")
step=$((step + 2000000))
arty_events+=(--uart-event "$step:$artifacts/arty-buttons.input")

step=$((step + 1000000))
for pin in 0 1 2 3; do
    arty_events+=(--litex-led-assert "$step:sysbus.cas.led$pin:off")
done
step=$((step + 1000000))
for mask in 1 3 7 15 0; do
    printf 'debug cas leds %s\r' "$mask" > "$artifacts/arty-leds-$mask.input"
    arty_events+=(--uart-event "$step:$artifacts/arty-leds-$mask.input")
    for pin in 0 1 2 3; do
        state=off
        if (( mask & (1 << pin) )); then
            state=on
        fi
        arty_events+=(--litex-led-assert "$((step + 1000000)):sysbus.cas.led$pin:$state")
    done
    step=$((step + 2000000))
done
arty_events+=(--litex-phy-event "$step:sysbus.eth.phy:0x4400")
arty_events+=(--uart-event "$((step + 1000000)):$artifacts/arty-mdio.input")
run_to_step_limit litex-arty "$arty" --max-steps "$((step + 3000000))" "${arty_events[@]}"
rg -aq '^H2U ' "$artifacts/litex-arty.uart.log"
diff -u <(printf '0\n1\n3\n7\nF\n0\n0 0\n1 1\n3 3\n7 7\nF F\n0 F\n0 0\n') <(
    tr -d '\r' < "$artifacts/litex-arty.uart.log" |
        awk '/>debug cas (switches|buttons read)$/ { if (getline) print }'
)

diff -u <(printf 'MDIO mode: 10Mbps / link: down\nMDIO mode: 100Mbps / link: up\n') <(
    tr -d '\r' < "$artifacts/litex-arty.uart.log" | rg '^MDIO mode:'
)

prepare_assets "$linux_single"
run_to_step_limit litex-linux "$linux_single" --max-steps 140000000
for marker in 'Linux version 5.0.14' 'sched_clock: 64 bits at 75MHz' 'Run /init as init process'; do
    rg -aqF "$marker" "$artifacts/litex-linux.uart.log"
done

prepare_assets "$litex"
printf 'root\r' > "$artifacts/litex-login.input"
printf 'cat /proc/cpuinfo\r' > "$artifacts/litex-cpuinfo.input"
run_to_step_limit litex-smp "$litex" --uart-event "600000000:$artifacts/litex-login.input" \
    --uart-event "675000000:$artifacts/litex-cpuinfo.input" --max-steps 725000000
for marker in 'OpenSBI v0.6' 'Litex/VexRiscv SMP' 'Linux version 5.0.9' \
    'smp: Brought up 1 node, 4 CPUs' 'Welcome to Buildroot' 'buildroot login:'; do
    rg -aqF "$marker" "$artifacts/litex-smp.uart.log"
done
rg -aq 'Platform Max HARTs\s+: 4' "$artifacts/litex-smp.uart.log"
for hart in 0 1 2 3; do
    rg -aq "^processor[[:blank:]]*: $hart\r*$" "$artifacts/litex-smp.uart.log"
done
prepare_assets "$renode_root/$i2s"
pcm=$(awk -F= '/^\$input_file\?=@https:/ { print $2 }' "$renode_root/$i2s")
printf '$output_file=@%s/echo.raw\ninclude @%s\n' "$(realpath "$artifacts")" "$i2s" \
    > "$artifacts/i2s.resc"
status=0
"$penv" run --script "$artifacts/i2s.resc" --platform-root "$renode_root" --asset-dir "$assets" \
    --stats > "$artifacts/litex-i2s.uart.log" 2> "$artifacts/litex-i2s.stderr.log" || status=$?
printf '%s\n' "$status" > "$artifacts/litex-i2s.exit"
if (( status != 0 )); then
    cat "$artifacts/litex-i2s.stderr.log" >&2
    exit 1
fi
cmp "$assets/${pcm##*/}" "$artifacts/echo.raw"
sha256sum "$assets/${pcm##*/}" "$artifacts/echo.raw" > "$artifacts/litex-i2s.sha256"
prepare_assets "$minerva"
python3 "$(dirname -- "$0")/validate-minerva-serial.py" "$penv" "$renode_root" "$artifacts"
prepare_assets "$nexys_video"
printf Q > "$artifacts/nexys-video-abort.input"
printf 'sdcardboot\r' > "$artifacts/nexys-video-sd.input"
printf 'root\r' > "$artifacts/nexys-video-login.input"
printf 'export PS1="$ "\r' > "$artifacts/nexys-video-prompt.input"
printf 'cat /etc/motd > /dev/tty0\r' > "$artifacts/nexys-video-motd.input"
run_to_step_limit nexys-video "$nexys_video" --max-steps 730000000 \
    --uart-event "35000000:$artifacts/nexys-video-abort.input" \
    --uart-event "40000000:$artifacts/nexys-video-sd.input" \
    --framebuffer-event "675000000:$artifacts/nexys-video-logo.ppm" \
    --uart-event "680000000:$artifacts/nexys-video-login.input" \
    --uart-event "690000000:$artifacts/nexys-video-prompt.input" \
    --uart-event "700000000:$artifacts/nexys-video-motd.input" \
    --framebuffer-event "720000000:$artifacts/nexys-video-motd.ppm"
for marker in 'Booting from SDCard in SD-Mode' "root login on 'console'"; do
    rg -aqF "$marker" "$artifacts/nexys-video.uart.log"
done
# Compare the MOTD frame and the 75x80 top-left logo ROI to the reference PNGs' RGB.
{
    printf '%s  %s\n' \
        c56966989d1d18b035b7a8f4f7442d94ff60ddc2beb822a405a877c4de06c7ff "$artifacts/nexys-video-motd.ppm" \
        | sha256sum --check
    python3 - "$artifacts/nexys-video-logo.ppm" <<'PYTHON'
from hashlib import sha256
from pathlib import Path
import sys

path = Path(sys.argv[1])
header = b"P6\n800 600\n255\n"
data = path.read_bytes()
if not data.startswith(header) or len(data) != len(header) + 800 * 600 * 3:
    sys.exit(f"{path}: expected an 800x600 RGB PPM")
pixels = memoryview(data)[len(header):]
roi = b"".join(pixels[row * 2400:row * 2400 + 225] for row in range(80))
if sha256(roi).hexdigest() != "e64eaa319a979cfb4bae39ee9e0c1ea2a04c688211a4ebe9527510bb4968ddfa":
    sys.exit(f"{path}: logo ROI differs from upstream")
print(f"{path} (75x80 logo ROI): OK")
PYTHON
} > "$artifacts/nexys-video.frames.log"
sd_root=$renode_root/scripts/single-node/litex_vexriscv_sdcard.resc
sd_fat=$(dirname -- "$0")/fixtures/litex-sd-fat.resc
prepare_assets "$sd_root"
printf 'root\r' > "$artifacts/sd-login.input"
printf 'ls -la /\r' > "$artifacts/sd-list.input"
run_to_step_limit litex-sd-root "$sd_root" --max-steps 550000000 \
    --uart-event "500000000:$artifacts/sd-login.input" \
    --uart-event "530000000:$artifacts/sd-list.input"
for marker in 'mmc0: new SD card at address 0001' 'mmcblk0: mmc0:0001 RENOD' \
    'blk_queue_max_hw_sectors: set to minimum 8' 'blk_queue_max_segment_size: set to minimum 4096' \
    'VFS: Mounted root (ext4 filesystem) readonly' 'Run /sbin/init as init process' 'buildroot login: root'; do
    rg -aqF "$marker" "$artifacts/litex-sd-root.uart.log"
done
sed -E 's/\x1b\[[0-9;]*m//g' "$artifacts/litex-sd-root.uart.log" | rg -a 'linuxrc -> bin/busybox' > /dev/null

prepare_assets "$sd_fat"
printf 'mount /dev/mmcblk0p1 /mnt; ls /mnt; cat /mnt/readme.txt\r' > "$artifacts/sd-read.input"
printf 'echo "This is a write test" > /mnt/readme2.txt; sync; umount /mnt; mount /dev/mmcblk0p1 /mnt; cat /mnt/readme2.txt\r' \
    > "$artifacts/sd-write.input"
run_to_step_limit litex-sd-fat "$sd_fat" --max-steps 620000000 \
    --uart-event "500000000:$artifacts/sd-login.input" \
    --uart-event "520000000:$artifacts/sd-read.input" \
    --uart-event "550000000:$artifacts/sd-write.input"
for marker in 'mmcblk0: mmc0:0001 RENOD 64.0 MiB' 'mmcblk0: p1' 'buildroot login: root'; do
    rg -aqF "$marker" "$artifacts/litex-sd-fat.uart.log"
done
diff -u <(printf 'This is a test file\nThis is a write test\n') <(
    tr -d '\r' < "$artifacts/litex-sd-fat.uart.log" | rg '^This is a (test file|write test)$'
)
# The file-backed card writes into a process-local overlay.
prepare_assets "$sd_fat"
linux_io=$(dirname -- "$0")/fixtures/litex-linux-io.resc
prepare_assets "$linux_io"
printf 'spidev_test -D /dev/spidev0.0 --speed 1000000\r' > "$artifacts/linux-io-spi.input"
printf 'i2cdetect -y 0\r' > "$artifacts/linux-io-i2c.input"
printf 'cat /sys/class/i2c-dev/i2c-0/device/0-0005/iio:device0/in_temp_raw\r' > "$artifacts/linux-io-temp.input"
printf 'echo 508 > /sys/class/gpio/export; echo 504 > /sys/class/gpio/export\r' > "$artifacts/linux-io-gpio.input"
printf 'echo 1 > /sys/class/gpio/gpio508/value\r' > "$artifacts/linux-io-led-on.input"
printf 'echo 0 > /sys/class/gpio/gpio508/value\r' > "$artifacts/linux-io-led-off.input"
printf 'cat /sys/class/gpio/gpio504/value\r' > "$artifacts/linux-io-button.input"
run_to_step_limit litex-linux-io "$linux_io" --max-steps 730000000 \
    --uart-event "490000000:$artifacts/sd-login.input" \
    --uart-event "520000000:$artifacts/linux-io-spi.input" \
    --uart-event "550000000:$artifacts/linux-io-i2c.input" \
    --uart-event "580000000:$artifacts/linux-io-temp.input" \
    --litex-temperature-event 600000000:i2c.si7021:36 \
    --uart-event "610000000:$artifacts/linux-io-temp.input" \
    --uart-event "630000000:$artifacts/linux-io-gpio.input" \
    --litex-led-assert 640000000:gpio_out.led:off \
    --uart-event "650000000:$artifacts/linux-io-led-on.input" \
    --litex-led-assert 660000000:gpio_out.led:on \
    --uart-event "670000000:$artifacts/linux-io-led-off.input" \
    --litex-led-assert 680000000:gpio_out.led:off \
    --uart-event "685000000:$artifacts/linux-io-button.input" \
    --litex-button-event 695000000:gpio_in.button:Toggle \
    --uart-event "700000000:$artifacts/linux-io-button.input" \
    --litex-button-event 710000000:gpio_in.button:Toggle \
    --uart-event "720000000:$artifacts/linux-io-button.input"
for marker in "root login on 'console'" 'spi mode: 0x0' 'bits per word: 8' 'max speed: 1000000 Hz (1000 KHz)' \
    'RX | FF FF FF FF FF FF 40 00 00 00 00 95 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF F0 0D' \
    '00:          -- -- UU -- -- -- -- -- -- -- -- -- --'; do
    rg -aqF "$marker" "$artifacts/litex-linux-io.uart.log"
done
diff -u <(printf '4368\n7724\n0\n1\n0\n') <(
    tr -d '\r' < "$artifacts/litex-linux-io.uart.log" | rg '^(4368|7724|[01])$'
)
nexys4ddr=$(dirname -- "$0")/fixtures/nexys4ddr-spi-sd.resc
prepare_assets "$nexys4ddr"
printf Q > "$artifacts/nexys4ddr-abort.input"
printf 'spisdcardboot\r' > "$artifacts/nexys4ddr-boot.input"
run_to_step_limit nexys4ddr-spi-sd "$nexys4ddr" --max-steps 100000000 \
    --litex-uart-legacy-console \
    --uart-event "35000000:$artifacts/nexys4ddr-abort.input" \
    --uart-event "80000000:$artifacts/nexys4ddr-boot.input"
for marker in 'Press Q or ESC to abort boot completely.' 'Cancelled' '> spisdcardboot' \
    'SD Card via SPI Initialising' 'Read FAT16 Boot Sector' '[mkfs.fat]' 'Root Directory' 'File 1 [README  .TXT]'; do
    rg -aqF "$marker" "$artifacts/nexys4ddr-spi-sd.uart.log"
done
tftp=$renode_root/scripts/single-node/litex_vexriscv_tftp.resc
prepare_assets "$tftp"
printf 'netboot\r' > "$artifacts/tftp-netboot.input"
printf '2 + 3\r' > "$artifacts/tftp-expr.input"
run_to_step_limit litex-tftp "$tftp" --max-steps 170000000 \
    --litex-uart-legacy-console \
    --uart-event "35000000:$artifacts/nexys4ddr-abort.input" \
    --uart-event "80000000:$artifacts/tftp-netboot.input" \
    --uart-event "150000000:$artifacts/tftp-expr.input"
for marker in 'Cancelled' '> netboot' 'Downloaded 218608 bytes from boot.bin over TFTP to 0x40000000' \
    'MicroPython v1.9.4-1431' '>>> 2 + 3'; do
    rg -aqF "$marker" "$artifacts/litex-tftp.uart.log"
done
tr -d '\r' < "$artifacts/litex-tftp.uart.log" | rg -a '^5$' > /dev/null
printf 'Murax UART echo, LiteX Ibex BIOS, LiteX Zephyr shell, LiteX Tock UART, Arty LEDs, switches, buttons and PHY status, LiteX single-hart and SMP Linux, I2S PCM, Nexys Video framebuffer, LiteX SD root and FAT16, LiteX Linux GPIO/SPI/I2C, Nexys4DDR SPI SD, and LiteX TFTP verified. Logs are in %s\n' "$artifacts"
