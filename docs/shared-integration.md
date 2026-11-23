# 共有コードの統合記録

統合の起点は `2b94f71`。対象は各担当ブランチの合格コミット以降の REPL と SystemBus の差分。上流 Renode は `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`、Infrastructure は `09ac7c563095c5410258ecc1b1a6b841a013db85`。

| 共有箇所 | 担当側の差分 | 統合状況 |
| --- | --- | --- |
| SystemBus のバイト検索 | ARM `5249abe`、STM32H7 `e45b5c6`、STM32F407 `86b5bf5` | 共通 Bus と RESC に実装。固定上流 `SystemBusTests.ShouldFindBytePattern` の隣接 1 MiB 領域を CLI で再現。CPU 指定の検索は残件 |
| SystemBus の一括転送 | ARM `bb1c38a`、STM32H7 `d749d8e`・`b18e8b6`、STM32F407 `da5d40f`・`43ef351` | メモリ限定モード、byte peripheral、別名とエラー伝播の整合が残件 |
| SystemBus の未マップアクセスとタグ | ARM `e1a1526`、STM32H7 `863d546`・`dc65132`、STM32F407 `216077f`、RV64 `516e99c` | 既存の統合版タグ優先順位と CPU 例外の差を照合する |
| CPU によるメモリ可視性 | RV64 `173385a`・`8705a56`・`3dc3755`、nRF52 `f09961a`・`539adde`、ARM `b374018` | コア・クラスタ限定範囲と既存のグローバル 64 ビットマップを照合する |
| REPL の属性と登録 | STM32H7 `6b71b37`・`6af6bb1`・`3d3a3ae`、STM32F407 `8e670e9`・`ab2bb0d`・`6c729a6`、nRF52 `f09961a`、RV32 `1a284f6` | 構文と登録検証の共通化候補。機種固有 CPU 型や属性は各ローダーで確認する |

`FindBytes` は単機の RESC と複数機のシミュレーション入口で同じパターン解析、検索、結果整形を使う。バス上のアドレス順に公開範囲だけを走査し、連続する登録間で検索を継続する。ARM の検索処理を統合版の RAM 登録形式に合わせ、両方式の別名と 4 GiB 以上をカバーした。STM32F407 側の補助 CLI が持つ検索は、統合側の RESC で再現できる。

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc cargo test --locked find_bytes
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc cargo test --locked
```

`cargo test` はユニット 209 件、CLI 18 件合格。ログは `~/.local/state/penv-dispatch/evidence/integration/shared-find-bytes-2b94f71/cargo-test.log`。

STM32H7 `3d3a3ae` と STM32F407 `8e670e9` の共通部分として、REPL の同じ宣言内の属性重複をオーバーレイの適用前に検出する。通常・複数機・nRF のローダーが共通の判定を使い、元のファイルと重複行を報告する。インデント形式とインライン形式を通常・複数機の CLI 入口で確認した。登録や init セクションの検証は別の後続差分として残る。

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc cargo test --locked renode_repl_reports_duplicate_attribute
```

対象テストのログは `~/.local/state/penv-dispatch/evidence/integration/shared-repl-duplicate-ca7e82f/focused.log`。既存 6 担当の回帰は `scripts/verify-release.sh` を使う。

## 起動前のメモリ保存

ARM `0737753` の Save/Clear/Load を統合版の Bus に接続した。単一 Cortex-M の実行前に RAM/ArrayMemory の内容とベクターテーブル基点を復元する。マップと入力を読み切ってからメモリを置き換え、既存の CPU 型と CPU 設定を保持する。切り詰められた保存ファイルと実行後の保存を CLI テストで確認した。

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc cargo test --locked --test cli -- --skip upstream_stm32f072
```

CLI 20 件と公開 STM32F072 Hello World ELF の Save、Clear、Load、mach set 0 後の UART 完了行を確認した。ログと RESC は `~/.local/state/penv-dispatch/evidence/integration/shared-boot-snapshot-b023006/`。固定 Neonkey の旧 USART と REPL の PerformanceInMips、実行中の ITSTATE と周辺機器の保存は残件。
