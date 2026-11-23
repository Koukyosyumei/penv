# RV64 AndeStar V5 の Quad MAC

固定 Renode の `tests/unit-tests/riscv-andestar-v5.robot` に沿い、`rv64gc_v_xandesvqmac`、VLEN 128 のベクトル積和命令を実装した。8・16ビットのソースから32・64ビットの累算値を更新する。符号指定、スカラーの SEW への切り詰め、マスク、実効 LMUL とレジスターグループの制約を固定 Infrastructure の `Andes_AndeStarV5Extension/QuadMac.cs` に合わせた。

| 上流ケース | 台帳 | 照合結果 |
| --- | --- | --- |
| Should Truncate Quad MAC Scalars On RV64 | pass | 同じ命令語と入力で、v12 の結果 `0x80`、VSTART=0、各ステップの PC が一致 |
| Should Advertise Quad MAC Configuration Registers | partial | RV64 側の `0xfc2` と `0xfca` のマスク値が一致。同一ケースには RV32 側の `0xfc2`、`0xfc3`、`0xfcb` の検査も含まれる |

変更前の Penv は `xandesvqmac` を認識せず、CPU の作成時点で停止した。固定フィクスチャにある RV32 専用の算術7ケースは台帳の対象外を維持した。その全21組を補足として RV64 に適用し、符号の組み合わせ、全7命令形式、マスクされた要素の保持を検証した。

## 再現

```sh
out=target/backlog-rv64-andes-qmac
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
  --fixture 'Should *Quad MAC*' --results-dir "$PWD/$out/upstream" -P 0 \
  > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ケース別ログと回帰の終了状態は `out` に保存した。

固定 Robot は対象2件が合格。Penv は同じバイナリで RV64 算術ケース、CSR の RV64 部分、補足7ケース、既存 AndeStar 14ケースを確認した。`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗。公開 RV64 virt イメージの OpenSBI、Linux、UART の root ログイン、`id -u` と `uname -m` の実行まで確認した。

## 残課題

設定 CSR ケースの等価合格には RV32 側の検査も必要。ベクトル命令の不正なグループ重複など、固定 RV64 算術ケースに含まれない条件の上流比較も残る。
