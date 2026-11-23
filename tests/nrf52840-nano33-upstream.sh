#!/bin/sh
# ArduCAMMini2MPPlus.robot and LSM9DS1.robot on the upstream Nano 33 BLE REPL.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
root=$2
out=$3
steps=${PENV_NANO33_MAX_STEPS:-1500000000}
mkdir -p "$out"
base=https://dl.antmicro.com/projects/renode
board=$root/platforms/boards/arduino_nano_33_ble.repl

fetch() {
    remote=$1
    file=$2
    if [ ! -f "$file" ]; then
        curl -fsSL "$base/$remote" -o "$file.part"
        mv "$file.part" "$file"
    fi
    printf '%s  %s\n' "${remote##*-}" "$file" | sha1sum --check
}

run() {
    log=$1
    shift
    status=0
    "$penv" run --platform "$board" --platform-root "$root" --max-steps "$steps" "$@" >"$log" 2>&1 || status=$?
    cat "$log"
    [ "$status" -eq 0 ] || [ "$status" -eq 2 ]
}

camera=$out/person-detection.elf
wand=$out/magic-wand.elf
fetch nrf52840--tf_person_detection.elf-s_7574264-cf1fccf46719c4a60e0df957a1304f17c5647011 "$camera"
fetch arduino_nano_33_ble--tf_magic_wand.elf-s_7482772-5722cd8b1dd7b040366cbc259f5175b62aa4496c "$wand"
failed=0
while read -r name bytes hash expected; do
    image=$out/$name.jpg
    log=$out/$name.log
    fetch "images/$name.jpg-s_$bytes-$hash" "$image"
    if run "$log" --nrf-camera-image "$image" "$camera" && python3 - "$log" "$expected" <<'PY'
import re
import sys
from pathlib import Path

output = Path(sys.argv[1]).read_text()
patterns = (
    "Attempting to start Arducam",
    "Starting capture",
    "Image captured",
    r"Reading \d+ bytes from Arducam",
    "Finished reading",
    "Decoding JPEG and converting to greyscale",
    "Image decoded and processed",
    r"Person score: (\d+) No person score: (\d+)",
)
for pattern in patterns:
    match = re.search(pattern, output)
    if match is None:
        sys.exit(f"missing ordered UART output: {pattern}")
    output = output[match.end():]
person, no_person = map(int, match.groups())
direction = int(sys.argv[2])
if direction * (person - no_person) <= 100:
    sys.exit(f"incorrect score difference: {person - no_person}")
PY
    then
        echo "$name: PASS"
    else
        echo "$name: FAIL"
        failed=1
    fi
done <<'CASES'
person_image_0 3853 7f2125e28423fa117a1079d84785b17c9b70f62d 1
person_image_1 3836 25216268e08894d0dda13107ad1ad5f537ad19c2 1
no_person_image_0 3787 3bed5184fbf005cbb0b6bf18e8885874ca7273bd -1
no_person_image_1 3910 81edff305382b42f3b98e3764921574e1b9142fe -1
CASES

steps=${PENV_NANO33_MAX_STEPS:-400000000}
while read -r motion marker; do
    log=$out/$motion.log
    if run "$log" --lsm9ds1-samples "$root/tests/peripherals/${motion}_rotated.data" "$wand" &&
        awk -v marker="$marker" '/Magic starts/ { started = 1 } started && index($0, marker) { found = 1; exit } END { exit !found }' "$log"
    then
        echo "$motion: PASS"
    else
        echo "$motion: FAIL"
        failed=1
    fi
done <<'CASES'
circle RING:
angle SLOPE:
CASES
exit "$failed"
