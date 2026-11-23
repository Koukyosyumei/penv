# RV64 ハート・クラスタ別メモリ登録

固定 Renode の `tests/unit-tests/per-core-registration.repl` を変更せず読み込めるようにしました。CPU とクラスタのメモリ登録はハートごとに解決します。同じアドレスでもハート集合が交わらない登録は別のバッキングを参照でき、ハートを指定しないロードやイメージ操作はグローバルな登録を使用します。固定 REPL のように同じ SiFive UART を同じアドレスで複数 CPU に登録する場合は、一つのデバイス状態をその CPU 間で共有します。

## ケース対応

| 固定 Robot の出典 | Penv の検証 |
| --- | --- |
| `per-core-registration.robot:97` Get Same Read From Common Memory | 元の REPL と公開 ELF を使用。共通メモリ `0x1400000` に `0xDEADF00D` を書いてから起動し、両コアの UART の値を順に照合。上流等価 |
| `per-core-registration.robot:106` Values Written By One Core Should Not Be Visible By The Other One | 元の REPL と公開 ELF を使用。同じアドレスで各コアの初期値 0、書き込み、読み返しを含む6行を順に照合。上流等価 |
| `per-core-registration.robot:227` Cluster Memory Should Only Be Visible To Cluster | Rust でハート集合ごとの共有・分離、重複拒否、グローバル別名の読み返しを照合。Monitor の読み書きは部分移植 |

固定 Renode は最初の2ケースを直接実行して合格しました。Penv は同じ UART 条件を検証し、観測を終えるため命令上限を設定しました。元の REPL に存在しない割り込み・SoC レジスターへのデータアクセスには固定 Renode と同じ警告継続モード `--renode-unmapped-data` を指定します。

`cargo test` は116件合格しました。前回のオフセット付きウィンドウの ELF 2本も通過し、固定 ACT 242本の結果は変更前と一致しました。

残る同スイートのケースには、Monitor の CPU コンテキスト指定、実行中の登録変更、CPU 別逆アセンブルが必要です。UART の別アドレスへの移動、複数クラスタで競合する登録、CPU スコープ付きページテーブルの検証も残っています。

## 再現

`$renode_work` は固定 Renode のビルド済みツリーです。終了状態とログは `target/backlog-rv64-per-core/` に保存しました。

```sh
out=target/backlog-rv64-per-core
mkdir -p "$out"
curl -fsSL https://dl.antmicro.com/projects/renode/multibus_test.elf-s_3718712-8ec6b7305242b1bfce702459d75ea02d04f00360 \
  -o "$out/multibus.elf"
printf '%s  %s\n' 8ec6b7305242b1bfce702459d75ea02d04f00360 "$out/multibus.elf" | sha1sum -c -
cargo test
cargo build --release
python3 scripts/verify-rv64-per-core.py "$out/multibus.elf" --logs "$out/final"
"$renode_work/renode-test" "$renode_work/tests/unit-tests/per-core-registration.robot" \
  --fixture 'Get Same Read From Common Memory' --results-dir "$out/upstream-common" -P 0
"$renode_work/renode-test" "$renode_work/tests/unit-tests/per-core-registration.robot" \
  --fixture 'Values Written By One Core Should Not Be Visible By The Other One' \
  --results-dir "$out/upstream-isolation" -P 0
```
