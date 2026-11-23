# RV64 Zcb と固定ベクトル CSR ケース

RV64 で `C` とともに `Zcb` を指定できます。12 通りの命令を既存の整数ロード・ストア、論理演算、符号拡張、乗算に展開します。各命令の追加 ISA 依存は既存のデコーダーが検査し、違法命令の `mtval` は元の16ビット命令を保持します。独立した `Zca` 指定、RV32 の Zcb、Zcmp と Zcmt は残課題です。[RISC-V Zc 仕様](https://docs.riscv.org/reference/isa/unpriv/zc.html)

## 検証

- 固定 Renode の `riscv-vector-csr.robot` 全12ケースが合格しました。Penv で固定 ISA 指定を使い、VS がオフの場合の例外3件、上位ビットと CSR の別名6件、飽和と VLEN 256 の VSTART を合わせて11件照合し、上流等価の合格に更新しました。
- `Should Mark MSTATUS.VS As Dirty` は保留です。上流は Monitor から `mstatus` 全体を照合するため、Monitor の表示値とゲストの CSR 読み出し値の対応を確認する必要があります。
- Zcb の全12命令を含む ELF、VSTART の ELF、VSADD の ELF は固定 ISA 構成で終了値 0 を確認しました。`cargo test` は121件合格しました。
- 固定の圧縮命令スイートに残る Zcmp/Zcmt と CV32E40P 構成はこの RV64 デコーダー変更の検証対象とは構成が異なります。

## 再現

ログと終了状態は `target/backlog-rv64-zcb/` にあります。`$renode_work` はビルド済み固定 Renode です。

```sh
out=target/backlog-rv64-zcb
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-vector-csr.robot" \
  --results-dir "$out/upstream-vector" -P 0
cat > "$out/vector-csr.repl" <<'REPL'
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    timeProvider: clint
    cpuType: "rv64imafd_zicsr_zifencei_c_v_zba_zbb_zbc_zbs_zfh_zacas_zcb"
    vectorRegisterLength: 256
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
REPL
sed '/vectorRegisterLength:/d' "$out/vector-csr.repl" > "$out/default-vector-csr.repl"
for source in rv64-zcb rv64-vector-length rv64-vector-bootstrap; do
  riscv-none-elf-gcc -march=rv64gcv_zba_zbb_zcb -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x2000,-e,_start,--section-start=.tohost=0x3000,--section-start=.data=0x4000 \
    "tests/fixtures/$source.S" -o "$out/$source.elf"
  platform="$out/vector-csr.repl"
  if [ "$source" = rv64-vector-bootstrap ]; then platform="$out/default-vector-csr.repl"; fi
  target/release/penv run --platform "$platform" --mtvec 0x1010 --robot-tohost 0x3000 \
    --max-steps 1000 "$out/$source.elf"
done
```
