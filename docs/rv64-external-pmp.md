# RV64 外部 PMP の Robot ケース

固定 Renode の `tests/unit-tests/riscv-external-pmp.robot` を、RV64 の4ケースに絞って直接実行した。Penv 側は同じ `rv64gc_zicsr`、外部 PMP 16エントリー、RAM 配置を使用する。Renode の実行ログで観測した初期 PC `0x1000` は Penv の REPL で明示した。

| 上流ケース | 台帳 | 照合内容 |
| --- | --- | --- |
| Should Access All 64-Bit Address Registers From Monitor | pass | 0〜15への書き込みと読み戻し、外部 PMP の有効化、無効アクセスの診断なし |
| Should Access All 64-Bit Address Registers From CPU | pass | 同じ CSR 命令列で a0 と t0 の各値、最終 a0=16、各ステップの PC、無効アクセスの診断なし |
| Should Access All 64-Bit Config Registers From Monitor | partial | 固定 Robot の書き込み、読み戻し、比較は `NOT RUN`。Penv の `pmpcfg0` の往復は補足検証 |
| Should Access All 64-Bit Config Registers From CPU | partial | 上流が実行した定数ロードを同じ PC 列で照合。CSR 命令と比較は固定 Robot で `NOT RUN`。Penv の `pmpcfg0` の往復は補足検証 |

固定 Robot の設定ループは `IN RANGE` に2引数を渡すため、開始と終了がともに2になり、本体を飛ばす。実行結果 XML でも両方の本体が `NOT RUN` だった。作業ツリー外の固定 Renode バイナリーに、開始を0、停止を2、刻みを2にした診断用コピーを渡すと、モニターの読み戻しは `0x03030303` に切り詰められ、CPU の比較キーワードは64ビット期待値の XML-RPC 転送でオーバーフローした。CPU の比較を `cpu GetRegister 10` と `cpu GetRegister 5` の出力経由に変更すると、a0 は期待値と一致したが t0 も `0x03030303` だった。固定 Infrastructure の `RiscVExternalPMP.ConfigCSRRead` では合成途中のシフト結果を `uint` に変換している。Penv が返す64ビット値とのこの差分が残る。

Penv では `--write-pmp-register cpu@pmpaddr0:0` と `--read-register-now cpu@pmpaddr0` を順に適用できる。外部 PMP が登録された CPU で、設定とアドレスの CSR をゲストと同じ状態に対して操作する。

## 再現

```sh
out=target/backlog-rv64-external-pmp
work=/tmp/penv-rv64-full
ref=/var/tmp/penv-rv64-act-upstream
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-external-pmp.robot" \
  --fixture '64-Bit' --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-external-pmp.py \
  --cc "$work/xpack-riscv-none-elf-gcc-15.2.0-1/bin/riscv-none-elf-gcc" \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-starlight.py "$starlight_assets" --ports 31454 31455 \
  --switch-port 31453 --upstream-cases --external-pmp --logs "$out/starlight" \
  > "$out/starlight.log" 2>&1
```

固定 Robot の設定ループを有効にする診断は、作業ツリーの `target` に置いたコピーで再現できる。ログは `upstream-config-loop.log` と `upstream-config-loop/robot_output.xml` に保存した。

```sh
sed -e 's|${PMP_ENTRIES}/8|${PMP_ENTRIES}//8|g' \
  -e 's/IN RANGE  \${cfg_reg_number}  \${step}/IN RANGE  0  \${cfg_reg_number}  \${step}/g' \
  ../renode/tests/unit-tests/riscv-external-pmp.robot > "$out/external-pmp-config-loop.robot"
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$PWD/$out/external-pmp-config-loop.robot" \
  --fixture '64-Bit Config Registers' --results-dir "$PWD/$out/upstream-config-loop" -P 0
```

XML-RPC の制限を避ける診断では、上のコピーで設定テストの `Register Should Be Equal` 2行を、それぞれ `Execute Command  cpu GetRegister 10` と `Execute Command  cpu GetRegister 5` の出力に対するローカルの `Should Be Equal As Integers` に置き換える。`upstream-config-text.log` と `upstream-config-text/robot_output.xml` に結果を保存した。

`starlight_assets` は README の上流 StarLight デモの公開素材。固定 Renode のビルドと ACT の生成は [ACT 終了通知と署名](rv64-act-equivalence.md) を参照。

`cargo test` は125件合格。固定 Robot は4件成功と報告し、そのうち設定アクセス2件のループ本体は上記のとおり未実行。アドレス2件の CLI 検証、設定2件の実行済み部分、補足の設定読み戻しは成功した。16エントリー構成で範囲外の `pmpaddr16` と奇数番号の `pmpcfg1` をモニターから書こうとすると、CLI がエラーを返すことも確認した。ACT 242 ELF は診断成功216本、診断失敗26本で、前回の TSV と一致した。

公開 StarLight イメージの外部 PMP 付き回帰は、OpenSBI、U-Boot、2台の Linux UART ログインまで通過した。両ゲストで `id -u` は `0`、`uname -m` は `riscv64`。ローカルスイッチ経由の ping は10送信、10受信だった。終了状態は各回帰の `*.status` に保存した。

## 隣接する未達

`riscv-amocas.robot` は RV32 と RV64 を同じプラットフォームに登録し、共通セットアップで RV32 CPU の実行モードと PC を設定する。Penv の RV64 ローダーは混在構成に未対応なので、RV64 単独で実行した MMIO の部分検証は引き続き部分移植となる。
