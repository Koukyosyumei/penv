# RV64 オフセット付きメモリ登録

固定 Renode の `tests/unit-tests/offset-bus-registrations.repl` と、同名 Robot スイートを対象にしました。REPL の複数行プロパティと、`BusRangeRegistration` の `address`、`size`、`offset`、`cpu` を読み取ります。単一 RV64 ハート向けのウィンドウは同じメモリの指定オフセットを参照し、CPU アクセスで同じアドレスのグローバル RAM より優先されます。コンテキストのないメモリ操作では通常のグローバル RAM を見ます。

## 検証と残課題

- 固定 Renode の Robot 全10ケースが合格しました。
- CPU 読み出し2ケースは、元の REPL、初期 PC `0x2000`、固定オフセット `0x4000`、同じ `ld` と期待レジスター値で ELF に移植しました。Penv 側はアサーション結果を `tohost` で検証し、2件とも合格しました。
- Sysbus の8ケースは Rust で byte／word／doubleword／quadword の読み書きとコンテキストの区別を確認しました。上流の Monitor コマンドを含むため、この段階では部分移植でした。上流の `Sysbus Should Read Byte` は Word のキーワードを呼ぶため、その挙動も台帳の出典に保持しています。
- `cargo test` は115件合格しました。固定 ACT 242本の結果は変更前と一致し、216本で自己署名と終了値 0 を確認しました。
- 複数ハートごとのウィンドウ、Monitor の CPU コンテキスト引数、デバイス登録への一般化は別の未達項目です。
- 固定 ACT の `Sm-00.elf` が署名で要求する `mstatus` ビット23・41は、固定 tlib の `op_helper.c` の `CSR_MSTATUS` 書き込みマスクにもありません。参照側で署名比較が異なる経路を通る理由の特定が残り、CSR 挙動の変更は保留です。[ACT の残課題](rv64-act-unmapped.md)

## 停止中の Sysbus ケースの後続検証

`--write-memory [CPU@]BASE:WIDTH:VALUE` を追加しました。`--read-memory-now` と組み合わせ、起動前に 1・2・4・8 バイトのグローバルまたは CPU コンテキスト付き RAM 操作を指定順に実行します。幅に収まらない値はエラーになります。

固定 Renode の Sysbus 8 ケースを直接実行し、Penv の CLI 移植も8件すべて通過しました。元の REPL と固定値を使い、Monitor の周辺機器相対アドレスは、同じバッキングのグローバルアドレスで指定しました。上流の `Sysbus Should Read Byte` は実際に Word を読みます。その挙動で照合しました。既存の CPU 読み出し用 ELF 2本も再実行して終了値 0 を確認しました。

実行中の UART 移動は保留です。固定 Robot は CPU の PC フックから UART の一方の登録だけを移動し、ELF を再ロードするマクロの後もその登録を保持します。Penv のリセット経路はマシンを組み直すため、そのままでは変更した登録を保持できません。実行中に共有 UART の CPU 別登録を移動する処理と、ELF のみを再ロードする経路が必要です。

`cargo test` は117件合格しました。ログと終了状態は `target/backlog-rv64-offset-monitor/` にあります。

```sh
out=target/backlog-rv64-offset-monitor
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/offset-bus-registrations.robot" \
  --fixture 'Sysbus Should *' --results-dir "$out/upstream" -P 0
python3 scripts/verify-rv64-offset-bus.py --logs "$out/penv"
```

## 再現

固定 Renode を `$renode_work` にビルドし、RV64 GCC を PATH に追加します。ログと終了状態は `target/backlog-rv64-offset/` に保存しました。

```sh
out=target/backlog-rv64-offset
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/offset-bus-registrations.robot" \
  --results-dir "$out/upstream" -P 0
python3 -c 'import sys; sys.stdout.buffer.write((0xfeedfacecafebeef).to_bytes(8, "little"))' > "$out/test-word.bin"
for pair in '81000000 80004000' '91000000 90004000'; do
  read -r window backing <<< "$pair"
  riscv-none-elf-gcc -DWINDOW=0x"$window" -march=rv64g -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-offset-bus.S -o "$out/window-$window.elf"
  target/release/penv run --platform ../renode/tests/unit-tests/offset-bus-registrations.repl \
    --load-binary "0x$backing:$out/test-word.bin" --robot-tohost 0x80010000 --max-steps 100 \
    "$out/window-$window.elf"
done
```
