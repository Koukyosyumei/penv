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

murax=$renode_root/scripts/single-node/murax.resc
litex=$renode_root/scripts/single-node/litex_vexriscv_smp.resc
ibex=$renode_root/scripts/single-node/litex_ibex.resc
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
        $1 == "sysbus" && ($2 == "LoadELF" || $2 == "LoadBinary") {
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
printf 'Murax UART echo, LiteX Ibex BIOS shell, and LiteX SMP Linux verified. Logs are in %s\n' "$artifacts"
