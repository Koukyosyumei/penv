# STM32F407 Discovery 上流ケース台帳

[ケース台帳](stm32f407-upstream-cases.tsv) は固定した上流から拾った候補を含む初版です。
`repository`、ファイル、行、ケース名、確認済みパラメーターを保持します。Robot の
テンプレートやキーワード内で展開されるパラメーター、NUnit の動的ソース、
共有スイートの機種適用判定には未確定の行があります。`review` はこの確認を必要とする行です。
`ported-pass` は移植した観測条件を通過したケース、`partial` は上流と観測条件に
差があるケース、`pending` は対象の移植待ち、`not-applicable` は機種・環境上の理由が
記録されているケースです。候補行数から全対象ケース数や全件通過を判断することはできません。

## 参照リビジョン

| リポジトリ | コミット | 調査内容 |
|---|---|---|
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` | Discovery 固有の全 13 件、Cortex-M・STM32 と REPL 関連の Robot/NUnit 候補 |
| Infrastructure | `09ac7c563095c5410258ecc1b1a6b841a013db85` | Peripheral Map 実装、STMCAN NUnit 3 件、タイマー・バス・DMA・GPIO・CRC の共有候補 |
| tlib | `c04522a9c886ba743c77a86b0c879103a2ef46d0` | Cortex-M エミュレーターの関連サブモジュール。このツリーに Robot/NUnit スイートはない |
| Berkeley SoftFloat | `6a9815f99719a24705e7a834f0ef34ac57441904` | 固定リビジョンを確認。このツリーに Robot/NUnit スイートはない |

元の Renode チェックアウトは読み取り専用で参照し、Infrastructure と tlib は別ディレクトリに
取得しました。台帳では同じ STM32F4 系でも F412G Tock のボード固有 ELF や
Cortex-M33 TrustZone 専用ケースに機種上の対象外理由を付けています。共有ケースでは
実装未達、ビルド依存、長い実行時間は対象外理由にしません。

## 再現

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
test "$(git -C "$RENODE_ROOT" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
cargo test
tests/stm32f407-upstream-smoke.sh target/full-upstream-stm32f407
```

個別の Peripheral Export は次の通りです。生成する JSON は Peripheral Map 1.0 の
ヘッダー、システムバス配下の別名、モデル名、登録アドレスを持ちます。

```sh
cargo build --release
target/release/penv peripherals export target/stm32f407-peripherals.json \
  --platform "$RENODE_ROOT/platforms/cpus/stm32f4.repl" --platform-root "$RENODE_ROOT"
```

スモークの出力には `Peripheral Export File Created` と
`Cortex-M4F context stacking: Test succeeded` が現れます。
元の公開 ELF は `fpu-v7m.log` に S0–S15 の復元結果を出力します。
`cargo test renode_stmcan_init_test` は、上流の初期化モード、送受信メールボックス、
フィルター制御と 28 バンクの比較を再現します。

## 未達

Discovery の PWM 2 件、RTC アラーム、FAULTMASK 2 件はスモークで動作を確認しています。
上流 PWM のマッチ条件と仮想時間、RTC のプロンプト連動とアラーム発火時刻、
FAULTMASK の TIM3 クロックおよび UART の待機開始からの無出力区間の等価性は未確定です。
フラッシュ消去はデータ結果を移植済みで、モードなしの消去要求に対する上流ログの検査が残ります。
共有スイートでは Cortex-M4F の NOCP・Fault 配送、UART ビット時間・パリティー、
別ボードの FPU/DSP 公開 ELF の適用判定と実行、NUnit の残りのパラメーター展開が残ります。
既存の FreeRTOS および Contiki の起動スモークは継続しますが、共有ケースの代替合格には数えません。
