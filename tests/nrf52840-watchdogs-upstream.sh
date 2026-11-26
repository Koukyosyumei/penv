#!/bin/sh
# NRF52840.robot hardware and task watchdogs, using the upstream ELFs.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$(realpath "$2")
mkdir -p "$3"
out=$(realpath "$3")
check=$(dirname "$0")/nrf52840-task-wdt-upstream.sh

asset() {
    file=$1
    remote=$2
    if [ ! -f "$file" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$file.part"
        mv "$file.part" "$file"
    fi
    printf '%s  %s\n' "${remote##*-}" "$file" | sha1sum --check
}

asset "$out/watchdog.elf" nrf52840--zephyr-watchdog.elf-s_889852-75a696a301c8439400645e646a322039963d74c8
asset "$out/task-wdt.elf" nrf52840--zephyr-task_wdt.elf-s_904904-91eadd958664ba0a009403a1a64f429dce3603a3

cat >"$out/watchdog.resc" <<RESC
mach create
machine LoadPlatformDescription @$root/platforms/cpus/nrf52840.repl
sysbus LoadELF @$out/watchdog.elf
sysbus.cpu PerformanceInMips 1
RESC

# Each of the two hardware waits and twenty task waits has an 8 s deadline.
"$check" "$out/watchdog.log" hardware -- "$penv" run --platform-root "$root"     --uart-steps --script "$out/watchdog.resc" --max-steps 16000000
"$check" "$out/task-watchdog.log" task -- "$penv" run --platform-root "$root"     --uart-steps --platform "$root/platforms/cpus/nrf52840.repl" --max-steps 16000000000 "$out/task-wdt.elf"
