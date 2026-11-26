#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
check() {
    "$penv" platform check --platform-root "$renode" "$@"
}
expect_error() {
    local expected=$1 source=$2 output status=0
    output=$(check --string "$source" 2>&1) || status=$?
    if [[ $status != 1 || $output != *"$expected"* ]]; then
        printf 'Unexpected exit %s for %s\n%s\n' "$status" "$source" "$output" >&2
        return 1
    fi
    printf '%s\n' "$output"
}
failed=0
for repl in platforms/boards/stm32f4_discovery-kit.repl \
    platforms/boards/stm32f4_discovery.repl platforms/cpus/stm32f4.repl; do
    if check --file "$renode/$repl"; then
        printf 'Loaded in a fresh machine: %s\n' "$repl"
    else
        failed=1
    fi
done
expect_error " Using 'invalid' does not exist" 'using "invalid"' || failed=1
if check --string ''; then
    printf 'Loaded empty platform string\n'
else
    failed=1
fi
expect_error ' Antmicro.Renode.Peripherals.CPU.TranslationCPU is an abstract type. Try constructing one of the following concrete types: ' \
    'cpu: CPU.TranslationCPU {}' || failed=1
exit "$failed"
