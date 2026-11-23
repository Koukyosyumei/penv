# RV64 のベクトル長とビット操作の照合

RV64 REPL の `vectorRegisterLength` を実装しました。固定 tlib の設定範囲に合わせて 64～65,536 ビットの2の累乗を受け付けます。V を有効にした CPU の既定値は512ビットです。指定した長さがレジスターバッキング、`vlenb`、`vstart` の書き込みマスク、`vl` の上限に反映されます。

## ケース対応と残課題

- 固定 Renode の `riscv-vector-csr.robot:197` が合格しました。Penv の ELF は同じ VLEN 256 で全ビットを `vstart` に書いて `0xff` を確認し、`vlenb=32`、`vl=64`（e8/m2）、`vsetvli` 後の `vstart=0` も確認しました。Penv 側の REPL は未実装の `Zcb` を除いたため、元ケースは部分移植です。実行中の Monitor からの VLEN 変更も残っています。
- 固定 `riscv-zbb-extension.robot` の RV64 RORI、RORIW、SLLI.UW の3ケースを直接実行しました。Penv の Rust 移植は固定 ISA、PC 0、同じ命令バイトを使い、全ステップの `a0` を照合しました。台帳では上流等価の合格です。
- 実行中の共有 UART の CPU 別登録移動は、ELF 再ロード時に変更した登録を保持する経路が必要です。[UART 移動の残課題](rv64-offset-bus.md)

`cargo test` は119件合格しました。ログと終了状態は `target/backlog-rv64-vlen/` にあります。

## 再現

```sh
out=target/backlog-rv64-vlen
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-vector-csr.robot" \
  --fixture 'Should Leave Enough VSTART Bits For Largest Possible Value' \
  --results-dir "$out/upstream-vector" -P 0
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-zbb-extension.robot" \
  --results-dir "$out/upstream-zbb" -P 0
cat > "$out/vector-csr.repl" <<'REPL'
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    timeProvider: clint
    cpuType: "rv64imafd_zicsr_zifencei_c_v_zba_zbb_zbc_zbs_zfh_zacas"
    vectorRegisterLength: 256
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
REPL
riscv-none-elf-gcc -march=rv64gcv -mabi=lp64d -nostdlib -static \
  -Wl,-n,-Ttext=0x2000,-e,_start,--section-start=.tohost=0x3000 \
  tests/fixtures/rv64-vector-length.S -o "$out/vector-length.elf"
target/release/penv run --platform "$out/vector-csr.repl" --robot-tohost 0x3000 \
  --max-steps 100 "$out/vector-length.elf"
```
