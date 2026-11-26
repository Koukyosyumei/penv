# RV64 ResetVector と停止中の CPU リセット

RV64 REPL の `resetVector` をサポートします。停止中は `--set-reset-vector CPU:ADDRESS`、`--set-pc CPU:ADDRESS`、`--reset-cpu CPU`、`--read-register-now CPU@NAME` を引数の順に使えます。初期 PC は明示的な PC 設定または ELF ロードまで ResetVector に追従します。CPU リセットは現在の ResetVector を保持し、CPU の初期状態と PC の追従を復元します。RAM と周辺機器は同じインスタンスに残します。

## 結果と残課題

固定 Renode の `riscv-unit-tests.robot:454` と Penv で12個の PC 観測値が一致しました。固定 Renode の既定値 `0x1000` は、既存 Penv プラットフォームの既定 PC に影響しないよう REPL で明示しました。別試行では ELF の入口 `0x2000` をロードしてから ResetVector を更新し、PC が入口値を保持することと、CPU リセット後に `0x1234` になることを照合しました。Rust では浮動小数点と整数レジスター、および構成済み `mstatus` の復元を確認しました。

`cargo test` は125件合格し、固定 ACT 242本の結果は変更前と一致しました。

CPU リセットは停止中のセットアップで使用します。実行中の Monitor 操作、周辺機器のリセット、ELF 再ロード後も移動済み UART 登録を保持するケースは未達です。

## 再現

ログと終了状態は `target/backlog-rv64-reset-vector/` にあります。`$renode_work` はビルド済み固定 Renode です。

```sh
out=target/backlog-rv64-reset-vector
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
  --fixture 'Should Properly Handle ResetVector After Creation And After Reset 64' \
  --results-dir "$out/upstream" -P 0
riscv-none-elf-gcc -DSINGLE -march=rv64gc_zfh -mabi=lp64d -nostdlib -static \
  -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-fdiv-zero.S -o "$out/entry.elf"
python3 scripts/verify-rv64-reset-vector.py --logs "$out/penv" --elf "$out/entry.elf"
```
