#!/usr/bin/env bash
# Port the three inline REPL cases in platform_descriptions.robot.
set -eu
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass an output directory}
mkdir -p "$logs"

failed=0
for name in invalid-using empty abstract-cpu; do
    case "$name" in
        invalid-using)
            repl='using "invalid"'
            expected=1
            message="Using 'invalid' does not exist"
            ;;
        empty)
            repl=''
            expected=0
            message=''
            ;;
        abstract-cpu)
            repl='cpu: CPU.TranslationCPU {}'
            expected=1
            message='Antmicro.Renode.Peripherals.CPU.TranslationCPU is an abstract type. Try constructing one of the following concrete types:'
            ;;
    esac
    status=0
    "$penv" check-platform --from-string "$repl" --platform-root "$renode" \
        > "$logs/$name.stdout.log" 2> "$logs/$name.stderr.log" || status=$?
    printf '%s\n' "$status" > "$logs/$name.exit"
    if (( status != expected )) || { [[ -n $message ]] && ! rg -Fq -- "$message" "$logs/$name.stderr.log"; }; then
        cat "$logs/$name.stderr.log" >&2
        printf 'FAIL %s\n' "$name" >&2
        failed=1
    else
        printf 'PASS %s\n' "$name"
    fi
done
exit "$failed"
