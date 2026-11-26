#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-thumb2-dp}
mkdir -p "$work"
elf="$work/instructions.elf"
sha=6adacbffaba09470a834f59b7e0ba21319b546c2
if [[ ! -f "$elf" ]]; then
    curl --fail --location --output "$elf.part" \
        "https://dl.antmicro.com/projects/renode/cortex-m-thumb2-data-processing-instr-test.elf-s_17032-$sha"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "$sha" "$elf" | sha1sum --check -
PENV_THUMB2_DP_ELF="$(realpath "$elf")" \
    cargo test renode_thumb2_data_processing_cortex_m4 -- --ignored --nocapture
