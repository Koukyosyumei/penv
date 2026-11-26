#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT SHELL_ELF" >&2
    exit 1
fi

penv=$1
resc=$2/scripts/single-node/nrf52840.resc
elf=$3
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

printf 'demo ping\r' > "$work/ping.txt"

status=0
"$penv" run --script "$resc" --platform-root "$2" \
    --uart-event "100000:$work/ping.txt" --reset-at 300000 \
    --uart-event "400000:$work/ping.txt" --max-steps 700000 "$elf" \
    > "$work/stdout" 2> "$work/stderr" || status=$?
cat "$work/stdout" "$work/stderr"
printf '\n'
if [ "$status" -ne 2 ]; then
    echo "Expected the Shell to run until the step limit (exit 2), got $status" >&2
    exit 1
fi

# Check the startup and post-command prompts on both sides of the reset.
if ! awk '
    BEGIN { count = split("uart:~$|demo ping|pong|uart:~$|uart:~$|demo ping|pong|uart:~$", expected, "|") }
    {
        while (match($0, /uart:~[$]|demo ping|pong/)) {
            if (substr($0, RSTART, RLENGTH) != expected[++seen])
                exit 1
            $0 = substr($0, RSTART + RLENGTH)
        }
    }
    END { if (seen != count) exit 1 }
' "$work/stdout"; then
    echo "Expected a prompt and a pong before and after reset" >&2
    exit 1
fi
