# 停止中の RV64 RAM 登録操作

`--max-steps 0` は REPL と指定されたセットアップを適用し、ゲスト命令を実行せずに終了します。`--load-binary`、`--read-memory-now`、`--move-ram CPU@OLD:NEW` は指定順に実行されます。`--read-memory` はセットアップ終了時の値を読みます。

移動先と移動元の範囲を確認してから、指定 CPU に専用のウィンドウだけを移動します。グローバル登録とクラスタ経由の登録は拒否し、バッキングとグローバル別名を保持します。CLI は移動元をアドレスで指定します。固定 Robot は対象の周辺機器名を指定します。

## 照合

| 固定 Renode Robot | Penv |
| --- | --- |
| `per-core-registration.robot:227` Cluster Memory Should Only Be Visible To Cluster | 元の REPL を使い、3つの書き込みと6つのコンテキスト付き・グローバル読み出し値を照合 |
| `per-core-registration.robot:248` Memory Should Only Be Movable Within Single CPU | グローバルとクラスタからの移動を拒否し、CPU 専用登録の移動は成功。Rust テストで拒否後の同一バスの状態も照合 |
| `per-core-registration.robot:257` Memory Move Should Only Affect Relevant CPU | 移動前の値、移動先の値、旧アドレスの 0、移動先への書き込み後のグローバル別名の値を照合 |

固定 Robot と Penv の両方で3ケースが合格しました。CPU 専用 RAM を停止中に操作する範囲を台帳の合格に更新しました。実行中の登録移動、UART 移動、Monitor API と CPU 別逆アセンブルは未達です。

## 再現

`$renode_work` はビルド済みの固定 Renode です。ログと終了状態は `target/backlog-rv64-move/` にあります。公開 ELF の準備は [ハート・クラスタ別メモリ登録](rv64-per-core.md) を参照してください。

```sh
out=target/backlog-rv64-move
mkdir -p "$out"
cargo test
cargo build --release
python3 scripts/verify-rv64-ram-move.py --logs "$out/penv"
for name in 'Cluster Memory Should Only Be Visible To Cluster' \
    'Memory Should Only Be Movable Within Single CPU' \
    'Memory Move Should Only Affect Relevant CPU'; do
  "$renode_work/renode-test" "$renode_work/tests/unit-tests/per-core-registration.robot" \
    --fixture "$name" --results-dir "$out/upstream-$name" -P 0
done
python3 scripts/verify-rv64-per-core.py "$multibus_elf" --logs "$out/per-core"
```
