# RV64 AndeStar V5 のビット分岐とビットフィールド

`rv64imac_xandesperf` に `nds.bbc`、`nds.bbs`、`nds.bfoz`、`nds.bfos` を追加した。デコードと演算は固定 Infrastructure の `Andes_AndeStarV5Extension` に合わせた。固定 `tests/unit-tests/riscv-andestar-v5.robot` の2ケースを Renode で直接実行し、Penv では同じ CPU、RAM、開始 PC `0x2000`、レジスター値、命令エンコードを使った。

| 固定ケース | Penv で照合した内容 |
| --- | --- |
| Should Test RV64 Bits Above Thirty One | 32・63ビット目に対する分岐4組。前方、後方、不成立それぞれの PC と a0 の保持 |
| Should Extract And Insert Full Width Bit Fields On RV64 | 6組の a2 と次の PC。64ビット全幅の取り出し、符号付き取り出し、ビット63への挿入、符号付き挿入 |

## 再現

```sh
out=target/backlog-rv64-andes-bits
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
for mode in branch field; do
  if [ "$mode" = branch ]; then
    filter='Should Test RV64 Bits Above Thirty One'
  else
    filter='Should Extract And Insert Full Width Bit Fields On RV64'
  fi
  PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
    "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
    --fixture "$filter" --results-dir "$PWD/$out/upstream-$mode" -P 0 \
    > "$out/upstream-$mode.log" 2>&1
done
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。実行ログと終了状態は `out` に保存した。

固定 Robot の2ケースと Penv の2ケースはすべて成功し、LEA 7ケースも再確認した。`cargo test` は125件合格。固定 ACT の診断は前回と一致し、216本成功・26本失敗だった。公開 RV64 virt イメージで OpenSBI から Linux を起動し、UART の root ログイン、`id -u` の `0`、`uname -m` の `riscv64` を確認した。

## 残課題

今回の分岐はビットのセット・クリア判定を実装した。比較定数で分岐する `nds.beqc` と `nds.bnec`、GP 相対ロード・加算、バイト検索などは未実装。対応する台帳のケースは従来の判定を維持した。
