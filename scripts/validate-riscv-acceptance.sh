#!/usr/bin/env bash
# Run from the penv checkout. Preserve the output directory as acceptance evidence.
set -euo pipefail

if (( $# < 2 || $# > 3 )); then
    echo "Usage: bash $0 RENODE_ROOT ARTIFACT_DIR [RISCV_TESTS_TAR_GZ]" >&2
    exit 1
fi
renode=$(realpath "$1")
mkdir -p "$2"
out=$(realpath "$2")
if [[ -n $(git status --porcelain) || -e $out/verification.log ]]; then
    echo "Use a clean penv checkout and a new evidence directory" >&2
    exit 1
fi
exec > >(tee "$out/verification.log") 2>&1
revision=$(git rev-parse HEAD)
printf 'penv %s\nRenode %s\n' "$revision" "$(git -C "$renode" rev-parse HEAD)"
if [[ -n ${PENV_BIN:-} ]]; then
    penv=$(realpath "$PENV_BIN")
else
    cargo test --locked > "$out/cargo-test.log" 2>&1
    cargo build --locked --release > "$out/cargo-build.log" 2>&1
    penv=$PWD/target/release/penv
fi
sha256sum "$penv"

archive=$out/riscv-tests.tar.gz
if (( $# == 3 )); then
    cp "$3" "$archive"
else
    curl -fsSL https://github.com/riscv-software-src/sail-riscv-tests/releases/download/2026-08-20/riscv-tests.tar.gz -o "$archive"
fi
printf 'eb24265e83ca0b68a24a513dff7e4535975e724fe07d71f3cf75fef87fd75bec  %s\n' "$archive" | sha256sum -c
mkdir "$out/elfs"
tar -xzf "$archive" -C "$out/elfs" --wildcards \
    './rv32u[imac]-[pv]-*' './rv32mi-*' './rv32si-*' './rv32uzicond-*'
virt=$renode/platforms/cpus/riscv_virt.repl
sed '/cpuType:/a\    allowUnalignedAccesses: true' "$virt" > "$out/virt-unaligned.repl"
sed 's/"rv32imac"/"rv32imac_zicsr_zifencei_zicond"/' "$virt" > "$out/virt-zicond.repl"
aligned=()
for elf in "$out"/elfs/rv32u[imac]-[pv]-*; do
    [[ $elf == *-ma_data ]] || aligned+=("$elf")
done
run_elfs() {
    local group=$1 count=$2 platform=$3
    shift 3
    [[ $# == "$count" ]]
    python3 scripts/validate-rv32-architecture.py --penv "$penv" --platform "$platform" \
        --artifacts "$out/$group" "$@"
}
run_elfs aligned 120 "$virt" "${aligned[@]}"
run_elfs unaligned 2 "$out/virt-unaligned.repl" "$out"/elfs/rv32ui-[pv]-ma_data
run_elfs privilege 22 "$virt" "$out"/elfs/rv32[ms]i-*
run_elfs zicond 4 "$out/virt-zicond.repl" "$out"/elfs/rv32uzicond-*

build_probe() {
    local name=$1
    shift
    clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
        -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld \
        "$@" "scripts/firmware/rv32-$name.S" -o "$out/$name.elf"
}
check_probe() {
    local name=$1 input=$2 status=0
    shift 2
    "$penv" run --platform "$virt" --platform-root "$renode" --max-steps 10000 \
        --uart-input "$input" "$out/$name.elf" > "$out/$name.uart" 2> "$out/$name.log" || status=$?
    [[ $status == 2 ]]
    diff -u <(printf '%s\n' "$@") "$out/$name.uart"
    printf 'PASS rv32-%s\n' "$name"
}
build_probe virt -DTEST_UART_IRQ
printf Q > "$out/virt.input"
check_probe virt "$out/virt.input" 'rv32-virt: NS16550 UART ready' \
    'rv32-virt: 10 MHz CLINT machine timer IRQ and mret OK' Q \
    'rv32-virt: NS16550 PLIC RX IRQ and mret OK'
build_probe priv
check_probe priv /dev/null 'rv32-priv: SRET with TSR trapped at the expected PC'
build_probe tvm
check_probe tvm /dev/null 'rv32-tvm: Sv32 supervisor SATP read/write and SFENCE.VMA gated; MRET recovery OK'
build_probe tlb
check_probe tlb /dev/null 'rv32-tlb: SFENCE.VMA refreshed Sv32 data and instruction translations'
build_probe counters
check_probe counters /dev/null 'rv32-counters: six MCYCLE/MINSTRET cases passed' \
    'rv32-counters: U-mode counter permissions passed'

mret_url=$(rg -o 'https://[^[:space:]]+' "$renode/tests/unit-tests/riscv-mret.robot")
curl -fsSL "$mret_url" -o "$out/mret.elf"
printf '257bec1c8918772e95eb9fdc1a432c852f052f99  %s\n' "$out/mret.elf" | sha1sum -c
cat > "$out/mret.repl" <<'REPL'
cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32im_zicsr"
ram: Memory.MappedMemory @ sysbus 0x10000
    size: 0x8000
forbidden: Memory.MappedMemory @ sysbus 0x2000
    size: 0x2000
REPL
status=0
"$penv" run --platform "$out/mret.repl" --stats --max-steps 43 "$out/mret.elf" \
    > "$out/mret.uart" 2> "$out/mret.log" || status=$?
[[ $status == 2 ]]
rg -q '^Limit, 42 instructions, .*pc=0x00010000$' "$out/mret.log"
echo 'PASS RV32 upstream Bare/PMP MRET'

bash scripts/validate-riscv-upstream.sh "$penv" "$renode" "$out/litex" | tee "$out/litex.log"
[[ $(git rev-parse HEAD) == "$revision" && -z $(git status --porcelain) ]]
echo 'PASS RV32 virt and LiteX acceptance'
