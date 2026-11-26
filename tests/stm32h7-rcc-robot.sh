#!/usr/bin/env bash
# Run the seven STM32H7 RCC Robot clock configurations on their fixed ELF.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
robot="$renode/tests/peripherals/STM32H7_RCC.robot"
mkdir -p "$assets" "$logs"

failed=0
for tag in CSI_4 HSE_8 HSE_8_CSS HSI_64 PLL_CSI_96 PLL_HSE_96 PLL_HSI_96; do
    work="$logs/$tag"
    mkdir -p "$work"
    url=$(awk -v tag="\${$tag}" '$1 == tag {sub(/^.*\//, "", $2); print $2}' "$robot")
    [[ -n $url ]]
    name=$url
    url="https://dl.antmicro.com/projects/renode/$name"
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat > "$work/case.resc" <<RESC
mach create "$tag"
machine LoadPlatformDescription @platforms/cpus/stm32h743.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
tester WaitUart sysbus.usart3 8 line "PROJECT EXECUTION SUCCESSFUL"
RESC
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 9000000 --uart-log-dir "$work" > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
