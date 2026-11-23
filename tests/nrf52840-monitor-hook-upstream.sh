#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 PENV RENODE_ROOT SHELL_ELF ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
root=$2
elf=$3
artifacts=$4
mkdir -p "$artifacts"

script="$artifacts/hook.resc"
cat "$root/scripts/single-node/nrf52840.resc" > "$script"
cat >> "$script" <<'RESC'

cpu AddHook `cpu PC` "monitor.Parse('log \"message from the cpu hook\"')"
RESC

status=0
"$penv" run --script "$script" --platform-root "$root" --max-steps 100000 "$elf" \
    > "$artifacts/uart" 2> "$artifacts/monitor" || status=$?
printf '%s\n' "$status" > "$artifacts/run.status"
if [ "$status" -ne 2 ]; then
    cat "$artifacts/uart" "$artifacts/monitor" >&2
    exit 1
fi
awk '
    /^message from the cpu hook$/ && !prompt { hook = 1 }
    /uart:~[$]/ { prompt = 1 }
    END { if (!hook || !prompt) exit 1 }
' "$artifacts/uart"
printf '%s\n' 'PASS hooks.robot:13-22 Should Parse Monitor in CPU Hook (Cortex-M4)'
