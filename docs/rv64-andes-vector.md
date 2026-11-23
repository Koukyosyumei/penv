# RV64 AndeStar V5 のベクトル BF16

固定 Renode の `tests/unit-tests/riscv-andestar-v5.robot` にある「Should Convert Vector BF16 On RV64」を移植した。変更前の代表実行では `xandesvbfhcvt` が未認識で CPU の作成時点で失敗した。固定と同じ `rv64gc_v_xandesvbfhcvt`、VLEN=128、RAM `0x0..0xfffff`、初期ステータス `0x2200` で照合した。

| 固定アサーション | Penv の結果 |
| --- | --- |
| CSR `0xfc2` と `0xfc7` | BF16 変換とベクトル設定ビット、最大浮動小数点幅のマスク値が一致 |
| `nds.vfwcvt.s.bf16` | BF16 `0x0001` を FP32 `0x00010000` に変換し、FFLAGS=0、VSTART=0 |
| `nds.vfncvt.bf16.s` | FP32 `0x00000001` を BF16 `0x0000` に変換し、FFLAGS=3、VSTART=0 |

固定ケースと同じ `vsetivli`、ユニットストライドのロード、変換、ストアを実行し、各ステップの PC と変換結果を照合した。変換は既存の浮動小数点処理を使用する。実効 LMUL、レジスターグループ境界、拡幅・縮幅時の重複制約に合わせて16↔32ビットの各要素を処理する。

## 再現

```sh
out=target/backlog-rv64-andes-vector
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
  --fixture 'Should Convert Vector BF16 On RV64' --results-dir "$PWD/$out/upstream" -P 0 \
  > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ケース別のログと回帰の終了状態は `out` に保存した。

固定 Robot と Penv で対象ケースは合格。同じ Penv バイナリで既存 AndeStar 13ケースも再確認した。`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗。公開 RV64 virt イメージで OpenSBI から Linux を起動し、UART の root ログイン、`id -u`、`uname -m` の実行まで確認した。

## 残課題

ベクトルメモリ命令は非マスク、単一フィールド、ユニットストライドの `vle` と `vse` を実装した。ストライド、インデックス、マスク付きアクセスおよびセグメント命令の対応は残る。AndeStar Quad MAC と関連設定 CSR も未達。
