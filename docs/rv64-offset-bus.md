# RV64 オフセット付きメモリ登録

固定 Renode の `tests/unit-tests/offset-bus-registrations.repl` と、同名 Robot スイートを対象にしました。REPL の複数行プロパティと、`BusRangeRegistration` の `address`、`size`、`offset`、`cpu` を読み取ります。単一 RV64 ハート向けのウィンドウは同じメモリの指定オフセットを参照し、CPU アクセスで同じアドレスのグローバル RAM より優先されます。コンテキストのないメモリ操作では通常のグローバル RAM を見ます。

## 検証と残課題

- 固定 Renode の Robot 全10ケースが合格しました。
- CPU 読み出し2ケースは、元の REPL、初期 PC `0x2000`、固定オフセット `0x4000`、同じ `ld` と期待レジスター値で ELF に移植しました。Penv 側はアサーション結果を `tohost` で検証し、2件とも合格しました。
- Sysbus の8ケースは Rust で byte／word／doubleword／quadword の読み書きとコンテキストの区別を確認しました。上流の Monitor コマンドを含むため部分移植です。上流の `Sysbus Should Read Byte` は Word のキーワードを呼ぶため、その挙動も台帳の出典に保持しています。
- `cargo test` は115件合格しました。固定 ACT 242本の結果は変更前と一致し、216本で自己署名と終了値 0 を確認しました。
- 複数ハートごとのウィンドウ、Monitor の CPU コンテキスト引数、デバイス登録への一般化は別の未達項目です。
- 固定 ACT の `Sm-00.elf` が署名で要求する `mstatus` ビット23・41は、固定 tlib の `op_helper.c` の `CSR_MSTATUS` 書き込みマスクにもありません。参照側で署名比較が異なる経路を通る理由の特定が残り、CSR 挙動の変更は保留です。[ACT の残課題](rv64-act-unmapped.md)

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
