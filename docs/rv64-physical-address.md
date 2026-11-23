# RV64 の物理アドレス上限

固定 Renode の `tests/unit-tests/memory-access.robot` は、RV64G / Priv1.10、RAM `0x0..0x2fff`、開始 PC `0x1000`、トラップベクター `0x2000` を使う。固定 tlib は RV64 に56ビットの物理アドレス幅を設定している。Penv は恒等変換経路で上限を確認し、範囲外の命令フェッチを中断する。データアクセスは診断と通常のアクセスフォールトを発生させる。CLI の `--translate-read` は Renode の `TranslateAddress <address> 0` に対応する。

| 固定ケース | 照合結果 |
| --- | --- |
| Should Report Illegal Address For Instruction Fetch | `li x1, 0x8000000000000000; jr x1` で対象 PC に到達し、固定 Renode と同じ診断で実行中断 |
| Should Report Illegal Address For Store | フィクスチャの命令は `lw x2, 0(x1)`。同じ診断が出て、トラップ先 `0x2000`、`mcause=5` を確認。Penv では `mtval` に対象アドレスが入ることも確認 |
| Should Throw Monitor Exception / RV64 | `TranslateAddress 0x8000000000000000 0` に対応する CLI 操作が `Failed to translate address` を返す。補足として56ビット上限内の最大アドレスは変換できる |
| Should Throw Monitor Exception / RV32 | 固定の CPU.RiscV32 / rv32i と `0x100000000` を使うため RV64 の台帳では対象外 |

CPU ケースは上流のアセンブリを GNU ツールチェーンで ELF 化する。Renode の短い `RunFor` はロードした命令列のステップに、トラップフックは同じトラップ先 PC と `mcause` の読み出しに置き換えた。フェッチケースの Renode は `Crash On Abort=False` でテストプロセスを維持する。Penv では対象 CLI が非ゼロで終了し、後続のモニター操作に進まないことを確認した。

## 再現

```sh
out=target/backlog-rv64-physical-address
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/memory-access.robot" \
  --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-physical-address.py \
  --cc "$work/xpack-riscv-none-elf-gcc-15.2.0-1/bin/riscv-none-elf-gcc" \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode のビルドと ACT の生成は [ACT 終了通知と署名](rv64-act-equivalence.md)、`virt_work` の準備は [README の RV64 virt 手順](../README.md) を参照。実行時の終了状態は同じログディレクトリーの `*.status` に保存した。

`cargo test` は125件合格。固定 Robot の3テストと CLI のRV64 3ケースは成功した。ACT 242 ELF は216本成功・26本失敗で、前回の診断 TSV と一致した。RV64 virt の公開ファームウェアは OpenSBI から Linux を起動し、UART で root にログインして `id -u` の `0` と `uname -m` の `riscv64` を確認した。

## 残課題

今回の固定フィクスチャはデータ読み出しのアドレス変換を使う。モニターの書き込み・命令フェッチ用の変換コマンドと、上流の時間・翻訳ブロック単位の観測はこの移植の範囲外。台帳の他グループと固定 ACT の診断失敗26本が残る。
