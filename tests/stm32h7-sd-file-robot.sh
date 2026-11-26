#!/usr/bin/env bash
# Port the three file-loading observations in SDCard.robot.
set -eu
cd "$(dirname "$0")/.."
renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
mkdir -p "$logs"
printf 'case\tstatus\n' >"$logs/cases.tsv"
failed=0
for case_name in existing nonexistent empty; do
    case "$case_name" in
        existing) file="$renode/tests/peripherals/SDCard.robot" ;;
        nonexistent) file=/doesntexist ;;
        empty) file='' ;;
    esac
    script="$logs/$case_name.resc"
    cat >"$script" <<RESC
mach create
machine LoadPlatformDescription @platforms/cpus/stm32h743.repl
machine SdCardFromFile "$file" sdmmc 0x1000
RESC
    status=0
    "$penv" run --script "$script" --platform-root "$renode" >"$logs/$case_name.log" 2>&1 || status=$?
    printf '%s\n' "$status" >"$logs/$case_name.exit"
    result=FAIL
    case "$case_name" in
        existing) if [[ $status == 0 ]]; then result=PASS; fi ;;
        nonexistent)
            if [[ $status != 0 ]] && rg -Fq "Could not find image file '/doesntexist'" "$logs/$case_name.log"; then result=PASS; fi ;;
        empty)
            if [[ $status != 0 ]] && rg -Fq 'No image file provided' "$logs/$case_name.log"; then result=PASS; fi ;;
    esac
    printf '%s\t%s\n' "$case_name" "$result" >>"$logs/cases.tsv"
    if [[ $result == FAIL ]]; then failed=1; fi
done
exit "$failed"
