#!/usr/bin/env bash
# Port the ordered ADC DMA and Crypto Sample UART checks from nucleo_h753zi.robot.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit states}
robot="$renode/tests/platforms/nucleo_h753zi.robot"
mkdir -p "$assets" "$logs"
failed=0
for tag in ADC_DMA_TEST CRYPTO_SAMPLE; do
    work="$logs/$tag"
    mkdir -p "$work"
    name=$(awk -v tag="\${$tag}" '$1==tag {sub(/^.*\//,"",$2);print $2}' "$robot")
    [[ -n $name ]]
    url="https://dl.antmicro.com/projects/renode/$name"
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat > "$work/case.resc" <<RESC
mach create "$tag"
machine LoadPlatformDescription @platforms/boards/nucleo_h753zi.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
RESC
    if [[ $tag == ADC_DMA_TEST ]]; then
        for name in test_adc_asynchronous_call test_adc_invalid_request test_adc_repeated_samplings \
            test_adc_sample_one_channel test_adc_sample_two_channels test_adc_sample_with_interval \
            test_task_different_priorities_sequences; do
            printf 'tester WaitUart sysbus.usart3 8 line "PASS - %s"\n' "$name"
        done >> "$work/case.resc"
    else
        for mode in ECB CBC CTR; do
            for direction in ENCRYPT DECRYPT; do
                printf 'tester WaitUart sysbus.usart3 8 line ": %s mode %s - Match"\n' "$mode" "$direction"
            done
        done >> "$work/case.resc"
    fi
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 57000000 --uart-log-dir "$work" \
        > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
