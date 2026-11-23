# RV64 AMOADD と単一ハート ID

RV64 REPL が単一 CPU の場合、非ゼロの `hartId` を指定できます。ハートの実行順と CPU スコープ付きバス登録にはインデックス 0 を使い、ゲストの `mhartid` と DTB 起動引数には指定値を渡します。複数 CPU の構成では従来どおり連続したハート ID が必要です。

`--step-cpu CPU` を追加しました。単一 RV64 CPU をセットアップ引数の途中で1ステップ動かし、次の引数でレジスターを書き換えたり、CPU とメモリを観測したりできます。

## ケース対応

固定 Renode の `riscv-amo-instructions.robot:141` と同 `:144` を直接実行し、Penv の ELF 移植で照合しました。元の ISA、`hartId: 1`、メモリサイズ、アドレス `0x100001000`、各命令とオペランドを使い、各ケースの5アサーションすべてが一致しました。追加 ELF で `mhartid=1` を確認しました。

ArrayMemory は Penv の RAM バッキングを通ります。今回の合格は、固定 AMOADD ケースの命令と観測値を対象とします。固定 AMOCAS の REPL は RV32 と RV64 の混在を要求するため、元の構成での実行が残ります。

`cargo test` は125件合格し、固定 ACT 242本の結果は変更前と一致しました。ログと終了状態は `target/backlog-rv64-amoadd/` にあります。

## 再現

```sh
out=target/backlog-rv64-amoadd
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-amo-instructions.robot" \
  --fixture '*MMIO Memory Location' --results-dir "$out/upstream" -P 0
for variant in HARTID WORD DOUBLE; do
  riscv-none-elf-gcc -D"$variant" -march=rv64gc -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x80000000,-e,_start tests/fixtures/rv64-amoadd.S -o "$out/$variant.elf"
done
python3 scripts/verify-rv64-amoadd.py "$out" --logs "$out/penv"
```
