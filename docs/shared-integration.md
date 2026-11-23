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

## 停止中のグローバル RAM 読み書き

`--write-memory ADDRESS:WIDTH:VALUE` と `--read-memory-now ADDRESS:WIDTH` は同じ Bus のグローバル RAM を引数順に操作する。幅は 1、2、4、8 バイト。負数はその幅内の二の補数、非負数は従来の RV64 構文と同じ符号なし範囲。 `--max-steps 0` と明示的な REPL で、ファームウェアなしの RAM 検証ができる。

`cargo test --locked` のユニット 209 件、CLI 21 件が合格。RV64 の固定値 `-1000000000000000000`、Cortex-M の `0x500` と `0xf0000000` を手元で再現した。証跡は `~/.local/state/penv-dispatch/evidence/integration/shared-memory-setup-b69d351/`。CPU スコープ付きアクセス、GDB RSP、取引状態は後続作業。

## UART 出力を待つ実行時操作

RV32 `fdacacf` と `885d642` を共有実行経路に取り込み、同じ照合機能から PHY、パネル、SI70xx 温度設定を実行する。パネルは同じ文字列の N 回目を指定できる。照合は UART 書き込みをまたいで継続し、操作は次のゲストステップの前に適用する。未適用のまま実行が終わると対象名付きエラーを返す。

`cargo test --locked` のユニット 210 件と CLI 21 件が通過した。固定公開 Arty ELF の Boot、LED、Switch、Button、PHY の5件を同じデバッグバイナリで確認した。証跡は `~/.local/state/penv-dispatch/evidence/integration/shared-uart-actions-96e0519/`。機種別の HS3001 と AD4011 には別のプロパティ接続が必要。

## RESC の幅別 SystemBus 操作

単一機の RESC で Byte、Word、DoubleWord、QuadWord の読み書きを受け付ける。1・2・4 バイトの 32 ビット空間内操作は既存の MMIO 経路を通り、8 バイトと 4 GiB 以上はグローバル RAM を使う。静的な読み書きだけのスクリプトはファームウェアなしで実行できる。既存のファームウェアに対する書き込みだけのスクリプトは、設定後も実行を続ける。

固定 `memory-assertions.robot` から RV32 9 件と RV64 3 件の生アドレス操作・比較を再現した。診断用 ISA とホスト側比較を使用。公開 STM32F407 FAULTMASK ELF では TIM3 のレジスタ設定後、4回以上のタイマー割り込み UART 行に到達。ログと再現スクリプトは `~/.local/state/penv-dispatch/evidence/integration/shared-sysbus-scalars-f89b35a/`。

## REPL の引用符と行コメント

通常・複数機・nRF の REPL ローダーが引用符内の区切りと行コメントを共通処理する。引用符内のセミコロンや波括弧、`@https://` の URI を保持する。引用符の復号は `using` と STM32H7 の SampleCsv に適用した。STM32F407 `5de13f0` の三つの入力について、共有復号器で元の期待値を確認した。MockCPU 全体の構築と複数行属性の汎用読み込みは残件。

`cargo test --locked` でユニット214件とCLI22件が通過し、固定公開 STM32F072 Hello World の ELF と HEX の UART 行も確認した。結果は `~/.local/state/penv-dispatch/evidence/integration/shared-repl-syntax-2cfdb6f/`。
