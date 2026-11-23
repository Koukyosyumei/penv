# RV64 AndeStar V5 の LEA 命令

固定 Renode の `tests/unit-tests/riscv-andestar-v5.robot` から `LEA` を検証する RV64 の7ケースを移植した。いずれも `rv64imac_xandesperf`、RAM `0x0..0xfffff`、開始 PC `0x2000` を使う。命令のエンコードは固定 Infrastructure の `Andes_AndeStarV5Extension.RegisterPerformanceInstructions` に合わせた。

| 固定 Robot のケース | 照合した命令と結果 |
| --- | --- |
| Should Execute LEA B/H/W/D ZE On RV64（4件） | `nds.lea.{b,h,w,d}.ze`。第2ソースの下位32ビットをゼロ拡張してから、0〜3ビットシフトする。第1ソースとの加算は64ビットで折り返す |
| Should Preserve Full Width LEA H/W/D On RV64（3件） | `nds.lea.{h,w,d}`。第2ソースの64ビット全体を1〜3ビットシフトして加算する |

固定ケースから読み込んだ21組すべてで、`a0` と `a1` を設定して1ステップ実行し、`a2` と PC `0x2004` を照合した。補足として、拡張指定を外した CPU では同じ命令が `mcause=2` でトラップし、`mepc=0x2000` になることを確認した。

## 再現

```sh
out=target/backlog-rv64-andes-lea
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
  --fixture 'Should *LEA* On RV64' --results-dir "$PWD/$out/upstream" -P 0 \
  > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開 Linux 素材は [README の RV64 virt 手順](../README.md)を参照。実行ログと終了状態は上記の `out` に保存した。

固定 Robot と Penv は7ケースすべて成功し、`cargo test` は125件合格した。固定 ACT 242 ELF は診断成功216本・失敗26本で、前回の TSV と一致した。公開 RV64 virt イメージで OpenSBI と Linux の起動、UART の root ログイン、`id -u` の `0` と `uname -m` の `riscv64` を確認した。

## 残課題

`xandesperf` 指定で今回実装したのは7種類の LEA。GP 相対ロード・加算、ビット分岐、ビットフィールド、バイト検索には別のデコードと演算が必要で、該当する固定ケースは引き続き台帳の未達に残る。
