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
        ($1 == "machine" && $2 == "SdCardFromFile") || $2 == "LoadPCM" {
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
arty_led_events=()
step=84000000
for mask in 0 1 3 7 15 0; do
    if (( step > 84000000 )); then
        printf 'debug cas leds %s\r' "$mask" > "$artifacts/arty-leds-$mask.input"
        arty_led_events+=(--uart-event "$((step - 1000000)):$artifacts/arty-leds-$mask.input")
    fi
    for pin in 0 1 2 3; do
        state=off
        if (( mask & (1 << pin) )); then
            state=on
        fi
        arty_led_events+=(--litex-led-assert "$step:sysbus.cas.led$pin:$state")
    done
    step=$((step + 2000000))
done
run_to_step_limit litex-arty "$arty" --max-steps 100000000 \
    --litex-phy-event 0:eth.phy:0x0 \
    --litex-phy-event 97000000:sysbus.eth.phy:0x4400 \
    --uart-event "98000000:$artifacts/arty-mdio.input" \
    --uart-event "70000000:$artifacts/arty-switches.input" \
    --litex-button-event 71000000:cas.switch0:Toggle \
    --uart-event "72000000:$artifacts/arty-switches.input" \
    --litex-button-event 73000000:cas.button0:Press \
    --uart-event "74000000:$artifacts/arty-buttons.input" \
    --litex-button-event 75000000:cas.button0:Release \
    --uart-event "76000000:$artifacts/arty-buttons.input" \
    --uart-event "78000000:$artifacts/arty-clear.input" \
    --uart-event "80000000:$artifacts/arty-buttons.input" \
    "${arty_led_events[@]}"
rg -aq '^H2U ' "$artifacts/litex-arty.uart.log"
diff -u <(printf '0\n1\n1 1\n0 1\n0 0\n') <(
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
printf 'Murax UART echo, LiteX Ibex BIOS, LiteX Zephyr shell, LiteX Tock UART, Arty LEDs, switches, buttons and PHY status, LiteX single-hart and SMP Linux, I2S PCM, Nexys Video framebuffer, and LiteX SD root and FAT16 verified. Logs are in %s\n' "$artifacts"
