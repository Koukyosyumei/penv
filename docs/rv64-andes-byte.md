# RV64 AndeStar V5 のバイト検索と比較

固定 Renode の `tests/unit-tests/riscv-andestar-v5.robot` にある「Should Search All Eight Bytes On RV64」を移植した。`rv64imac_xandesperf` と同じ RAM・PC で、固定 Infrastructure の `Andes_AndeStarV5Extension` に合わせて4命令をデコードする。

| 命令 | 検索条件 |
| --- | --- |
| `nds.ffb` | 第2ソースの最下位バイトと最初に一致する位置 |
| `nds.ffzmism` | 第1ソースがゼロ、または対応バイトが不一致になる最初の位置 |
| `nds.ffmism` | 最初の不一致 |
| `nds.flmism` | 最後の不一致 |

Penv のリトルエンディアン RV64 では最下位バイトの位置を0と数える。見つかった位置から8を引いた値を返し、検索条件を満たす位置がなければ0を返す。固定ケースの全6入力について、`a2` と次の PC `0x2004` を照合した。

## 再現

```sh
out=target/backlog-rv64-andes-byte
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
  --fixture 'Should Search All Eight Bytes On RV64' --results-dir "$PWD/$out/upstream" -P 0 \
  > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ログと終了状態は `out` に保存した。

固定 Robot と Penv の対象ケースは合格し、Penv の既存 AndeStar 11ケースも同じバイナリで再確認した。検証スクリプトはケースごとの合否を標準出力に記録し、失敗しても後続ケースを実行する。`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗。公開 RV64 virt イメージで OpenSBI から Linux を起動し、UART の root ログインと `id -u`、`uname -m` を確認した。

## 残課題

同じスイートの RV64 BF16 と Quad MAC は未達。BF16 はスカラーの NaN ボクシング、丸めと例外フラグに加えて、ベクトル側の変換処理が必要。Quad MAC は演算と設定 CSR の照合が残る。
