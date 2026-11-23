# RV64 のスタックアクセス記録

固定 Renode の `tests/unit-tests/riscv-stack-access-hook.robot` の21ケースを移植した。同じ `rv64gc_V`、CLINT 周波数、RAM、PC `0x1000` と SP `0x3000` を使う。Penv の REPL で `traceStackAccess: true` を指定すると、ベースレジスターが `sp` のアクセス直前に、実効アドレス、ビット幅、書き込みかどうかを標準エラーに記録する。この記録が固定 Robot の `AddPreStackAccessHook` のログ出力に対応する。

| 固定ケース群 | 件数と照合内容 |
| --- | --- |
| 整数ロード・ストア | 9件。幅8/16/32/64と、64バイトのオフセット付きロード |
| LR/SC と浮動小数点ロード・ストア | 4件。32ビットの LR/SC と64ビットの FLD/FSD |
| RVV のユニットストライドとストライド付きアクセス | 8件。幅8/64、VL=8、読み書き両方向。固定 Robot が検査する先頭7要素に加え、Penv では8要素すべての記録を照合 |

変更前は代表の LB でイベントが出なかった。新しいストライド付きアクセスは符号付きのバイトストライドをアドレスに加え、既存のユニットストライド命令と同じ要素処理を使う。検証スクリプトは固定 Robot の命令列を GNU ツールチェーンで ELF 化し、同じ Penv バイナリで一括実行する。Penv 側では浮動小数点とベクトルを有効にしてから命令列を開始する。

## 再現

```sh
out=target/backlog-rv64-stack-access
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-stack-access-hook.robot" \
  --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-stack-access.py ../renode/tests/unit-tests/riscv-stack-access-hook.robot \
  --cc "$work/xpack-riscv-none-elf-gcc-15.2.0-1/bin/riscv-none-elf-gcc" \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/andes" > "$out/andes.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ケース別ログと終了状態は `out` に保存した。

固定 Robot と Penv で対象21ケースが合格した。AndeStar 回帰は補足を含む24チェックが合格し、`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗だった。公開 RV64 virt イメージは OpenSBI から Linux を起動し、UART の root ログイン、`id -u`、`uname -m` の実行を確認した。

## 残課題

任意の Python スタックアクセスコールバックと、マスク・インデックス・セグメント付き RVV メモリアクセスは未対応。固定ケースのコールバックはログ出力だけを行うため、REPL の記録設定で検査内容を再現した。
