# STM32F407 Discovery 上流ケース台帳

[ケース台帳](stm32f407-upstream-cases.tsv) は固定上流のリポジトリ、ファイル、ソース行、
ケース名、パラメーター、フィクスチャ、判定根拠または証拠を記録します。

| 出典 | 対象実行ケース | 上流等価合格 | 部分移植 | 未達 | 対象外 |
|---|---:|---:|---:|---:|---:|
| Renode | 510 | 352 | 0 | 158 | 240 |
| Infrastructure | 217 | 51 | 0 | 166 | 7 |
| **合計** | **727** | **403** | **0** | **324** | **247** |

分母は対象の具象行です。未判定行は 0 件です。BusFault の命令別行への参照を
保持する親行 1 件（`expanded`）には実行件数を割り当てません。
`ported-pass` は既存の等価検証、`partial` は観測条件または検査項目に差がある移植、
`pending` は等価検証がまだない対象ケースです。
今回の回帰で実行した FreeRTOS と Contiki のローカルスモークは、この集計に追加していません。

## 適用・展開の監査

- 実フィクスチャからボード、CPU、ISA、実行に必要な配線を確認しました。F429 の
  名前を持つネットワーク ELF 2 本は実際には Discovery REPL 2 台と仮想スイッチで
  動くため、TCP と両 UART を検査する 1 構成として残しました。ADC の外付け
  AD4011 は、上流オーバーレイが F407 SPI1 に接続する 4 ケースを対象としました。
- 単一 CPU の排他アクセス、Thumb DSP、セミホスティング、MPU、UART テスター、
  診断やパーサー、バス、タイマー、GPIO、DMA、CRC の共有テストを残しました。
  合成メモリーやモックはフィクスチャの一部です。異機種の CPU を使うケースでも、
  同じ刺激と観測を M4F で検査できるものは対象です。固定された TrustZone の別名、
  MVE 命令、A/R プロファイル固有命令、複数コアの同一バス上の隔離などを
  必要とする場合は、台帳行に別構成と対象外の理由を記載しました。
- Robot の独立した入力組は展開しました。例として Cortex-M の LLVM 往復は
  4 命令、セミホスティングの一括実行は 22 組に分かれます。状態を共有する
  検査手順は 1 構成に保持しました。NUnit は DMA の 3×3×4×4、SystemBus の
  2×2、REPL 数値パーサーの 3・8・4 の組を個別行にしています。
- `Should Load Repls` は固定上流の列挙とブラックリストを照合しました。
  CPU、Discovery、Discovery kit の 3 REPL が対象です。
  `stm32f4_discovery-additional_gpios.repl` と `stm32f4_discovery-bb.repl` は
  上流の単独ロードの列挙から除かれ、kit の `using` で読まれます。
  F412/F429 SoC の各変種には対象外の根拠を記載しました。列挙に含まれる
  その他の REPL は F407 をロードしない別のボードまたは SoC の構成です。
- Robot の継続行 `...` をケースとして拾った 32 行と、同じ BXNS / TrustZone
  構成の重複を除きました。異なる上流ファイルのケースや異なるパラメーターは、
  同じ移植ランナーを使う場合でも出典を保持しています。

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

## 既存スモークの時刻条件

- PWM 2 件は未変更の CPU REPL に上流と同じ PA6 → PD12 を追加します。
  CPU は固定 Renode の既定 100 MIPS、タイマーは REPL の 10 MHz、SysTick は 72 MHz です。
  周波数で最初に一致した行を読み飛ばし、次の測定値を Robot と同様に整数部で比較します。
  確認値は通常版が 1000 Hz・33.0%、4 チャネル版が 1001 Hz・32.9% でした。
- RTC は 32 kHz の上流オーバーレイを使用し、プロンプト行の次の実行ステップで `s` を渡します。
  `rtc-alarm.tsv` の行末時刻から Renode の `Elapsed Virtual Time` の秒フィールドを再現し、
  2・4・6・8 秒を確認します。4 行目の後は UART の未改行出力も含め、3 秒間無出力を要求します。
- FAULTMASK 2 件は RCC の事前書き込みを除き、上流 REPL の TIM3 周波数を使用します。
  `faultmask.tsv` の `Sleeping` 行末から次の UART バイトまで 1 秒以上と、
  マスク解除後 3 回、例外復帰テストで 4 回の割り込み出力を確認します。

`--uart-transcript PATH` の各行は仮想開始ステップ、終了ステップ、UART テキストの TSV です。
RTC と FAULTMASK は既定の 1 ステップ/µs で上流の仮想時刻と UART 観測条件を検査します。
この 2 スイートの CPU 命令実行速度は固定 Renode の既定 100 MIPS と異なります。
100 MIPS の個別検証には下記の時刻条件付きランナーを使います。
スモークは既知 ELF の実行を仮想ステップ上限で区切ります。Robot の端末待機の既定タイムアウト
（待機ごとに 8 秒）は汎用機能として再実装していません。

## 未達

未達 324 件には、共有フォールト配送、汎用 UART テスター、GDB・モニター・
スナップショットの操作、実ファームウェアの DSP/FPU/MPU、Ethernet、NUnit
フィクスチャの等価検証が残ります。全件合格ゲートの条件は未達です。

### 適用監査時の再検証

`target/stm32f407-scope-audit/` に回帰ログを保存しました。
`cargo test` は 76 件が通過しました。`faults.log` は Cortex-M4F ELF 16 件の
UART 判定を、`freertos.log` と `fpu.log` は未変更 Discovery 定義での TIM2 キュー、
UART エコー、CAN ループバック、IWDG 再起動と FPU コンテキスト判定を記録しています。
`contiki.log` には `Contiki 3.x started.` と `UDP server started`、
`zephyr.log` には `Hello World! stm32f4_disco`、
`upstream-fpu.log` には公開 ELF の `Test succeeded` が出ました。
Contiki、Zephyr、公開 Cortex-M4 ELF の起動は上記のスモークの同名手順を個別に再実行しました。
PWM など今回再実行していない既存合格は台帳の既存証拠を参照します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-scope-audit
mkdir -p "$work"
cargo test >"$work/cargo-test.log" 2>&1
tests/cortex-m4f-faults.sh "$work/faults" >"$work/faults.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream >"$work/freertos.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/fpu" --fpu >"$work/fpu.log" 2>&1
```

### Cortex-M4F の精密 BusFault と NOCP

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/cortex-m4f-faults.sh target/cortex-m4f-faults
tests/stm32f407-freertos-smoke.sh target/stm32f407-fault-upstream --upstream
tests/stm32f407-freertos-smoke.sh target/stm32f407-fault-cm4f --fpu
```

Clang と LLD で上流のインラインアセンブリを Cortex-M4F 用 ELF に組み立てます。
CPU モデルを ARMv7E-M に変更し、Robot がモニターから行っていた設定と検査を ELF 内で
行います。13 個の ELF はそれぞれ UART に `BusFault OK`、`NOCP`、または `OK` を
出し、レジスター・スタック値が違えば FAIL を出します。

- 通常の精密 BusFault は命令アドレス `0x200`、SP `0xfe0`、R1 `0x11`、
  R2 `0xa5a5a5a5`、CFSR `0x8200` を確認します。BFAR はモック周辺機器の
  `0x100000`、未マップ読み出しでは `0x200000` です。
- 無効化した BusFault と同じ優先度の実行中ハンドラーからの BusFault は HardFault に
  エスカレーションし、HFSR は `0x40000000` です。後者は `0xfd8` に
  BusFault ハンドラーの命令アドレス `0x300` が保存されます。前者は HFSR の W1C も確認します。
- 連続実行の読み書きは R10 と R11 の PC が一致することを照合します。FPv4-SP の
  `VSTR/VLDR`（S0/D0）は拡張フレームの SP `0xf98` と同じ精密フォールト値を検査します。
- CPACR が初期値のときは UsageFault に入り、`0xe000ed2a` の半ワードが `0x0008`、
  スタックに障害命令の PC が保存されることを確認します。フルアクセス設定時は
  `VLDR → VCVT.S32.F32 → VCVT.F32.S32 → VSTR` が完了し、`OK` を出します。

未実装の FP オペコードと特権限定 CPACR アクセスは残件です。

### Cortex-M4F の命令フェッチと例外ベクター

上記の `tests/cortex-m4f-faults.sh` はフェッチと IRQ ベクターの ELF 3 件を追加で実行し、
各 ELF の UART に `Fetch fault OK` が出ることを照合します。
Rust テストはリセットベクター、初期 MSP、NMI・HardFault の取得失敗と、
NMI 進入時にベクター取得とスタック書き込みが同時に失敗するケースの状態を検査します。

- Thumb-2 の第1・第2半ワードの取得失敗は `CFSR.IBUSERR=1` を記録し、
  命令先頭の PC を保存します。BFAR と BFARVALID は変更しません。BusFault が無効な場合は
  HardFault に配送し、`HFSR.FORCED=1` を記録します。
- IRQ16 のベクター取得失敗は `HFSR.VECTTBL=1` を記録して HardFault に配送します。
  SP は `0xfe0`、CFSR は `0`、元の IRQ は ISPR に残り IABR では非アクティブです。
- NMI・HardFault のベクター取得失敗では `DHCSR.S_LOCKUP` と例外番号、SP、LR、
  `HFSR.VECTTBL` を検査します。同時スタック障害では `CFSR.STKERR` も記録します。
  リセットベクターまたは初期 MSP の取得失敗は SP `0`、LR `0xffffffff`、IPSR `0` と
  HardFault のアクティブ状態を検査します。

上流 Robot の Cortex-M55 フィクスチャは、Cortex-M4F の EXC_RETURN
（スレッド・MSP の基本フレームでは `0xfffffff9`）に置き換えました。
上流の SHCSR に現れる NMI・HardFault のアクティブ状態は Rust テストで内部状態を照合します。
初期 MSP がゼロのリセットでは、2 ステップ目の INVSTATE ロックアップと DHCSR の
CPU・デバッガー別の読出しを確認します。命令実行時のロックアップ解除は下記を参照してください。

## Robot の時間条件を移植した 7 件

`--uart-check PATH` はタブ区切りの `wait 秒 文字列`、`send 文字列`、
`second 秒`、`idle 秒` を順に実行します。`wait` は完了行の部分一致、
`send` は次の実行ステップで改行なしの入力、`second` は経過仮想時刻の秒整数部、
`idle` は未改行バイトも含む無出力を検査します。テスト計画が全行通過すると終了コード
0、未一致・タイムアウト・無通信中の出力では非 0 です。診断ログには仮想時刻を記録します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
test "$(git -C "$RENODE_ROOT" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
work=target/stm32f407-timed-tests
mkdir -p "$work"
(
    cargo test -- --nocapture >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    cargo test renode_f407_flash_ignores_erase_without_mer_or_ser -- --nocapture >"$work/flash.log" 2>&1 &&
        rg -qF 'Tried to erase flash, but MER and SER are reset. This should be forbidden, ignoring...' "$work/flash.log" &&
        rg -qF 'FLASH[0x08000000] = 0xDEADBEEF before and after erase request' "$work/flash.log"
    echo "$?" >"$work/flash.status"
    tests/stm32f407-timed-tests.sh "$work" >"$work/timed.log" 2>&1
    echo "$?" >"$work/timed.status"
) &
```

`PENV_F407_TIMED_RENODE_ROOT` を指定すると WFE・BASEPRI・フラッシュの Rust テストは
固定 upstream REPL をロードします。ゼロ MSP は上流の合成フィクスチャを Cortex-M4F に
移植します。期待ログは `WFE: 0 at 0.001 s`、`BASEPRI: priority boost 0x50 before reset, 0x100 after reset`、
`zero MSP: two steps` と、対象フラッシュ要求と同時に出る警告です。フラッシュの
Robot は要求の直後に既に出ているログを最長 3 仮想秒待ちます。移植は同期書き込みの
その診断を照合し、同じ書き込みの前後で `0xDEADBEEF` を確認します。

公開 ELF 3 本は SHA-1 を照合し、100 MIPS、上流の周辺クロック、USART2、行ごとの
8 仮想秒の待機で実行します。RTC はプロンプト後に `s` を 1 バイト送信し、
アラームの時刻 2・4・6・8 秒とその後 3 秒の無通信を検査します。FAULTMASK は
`Sleeping` の後 1 秒の無通信と解除後 3 回の IRQ、例外復帰のケースは 4 回の IRQ を検査します。
実行は `rtc` または `faultmask` を第 2 引数に渡すと個別に選択できます。

今回の検証ログでは Rust の 81 テスト（`cargo-test-final.log`）、Cortex-M4F フォールト ELF 16 件、FreeRTOS
FPU 回帰、フラッシュ診断の終了状態が 0 でした。`terminal/` には UART チェック
の小型 ELF と、成功、未改行バイトによるアイドル違反、8 秒の待ち時間切れのログがあります。
同じ成功・失敗条件は CLI 統合テストにも追加しました。

FAULTMASK 2 件のタイムスタンプは `faultmask-check.log` と
`faultmask-exit-check.log` にあります。1 秒の無通信、解除後の IRQ 3 回、例外復帰後の
IRQ 4 回を確認しました。最初の実行ではシェルが動作中のファイル編集の影響を受け、
両 ELF が計画を完了した後にラッパーが 127 で終了しました。各 ELF の SHA-1、全操作、
タイムスタンプを突き合わせた `faultmask-evidence.log` の終了状態は 0 です。
最終版のラッパーは `bash -n` でも確認しました。

最後の部分移植だった RTC アラームは、公開 RIOT ELF の SHA-1 と固定上流の
フィクスチャを照合し、100 MIPS で全操作の合格を確認しました。
アラームは 2.000070、4.000070、6.000070、8.000070 仮想秒に出て、
11.000070 仮想秒まで UART 出力がありませんでした。実行と証拠照合はともに終了状態 0 です。
`target/stm32f407-timed-tests-rtc/` の `rtc-check.log`、`rtc.log`、`rtc.status`、
`rtc-evidence.log`、`rtc-evidence.status` に結果があります。台帳のみの更新のため、
完了した ELF 実行を再利用しました。

同じケースだけをビルドから再現するには、上記の環境変数を設定して次を実行します。

```sh
work=target/stm32f407-timed-tests-rtc-repro
mkdir -p "$work"
(
    tests/stm32f407-timed-tests.sh "$work" rtc >"$work/run.log" 2>&1
    rc=$?
    printf '%s\n' "$rc" >"$work/rtc.status"
    exit "$rc"
) &
```

## Cortex-M4F の例外スタック保存・復元フォールト

固定上流の `tests/unit-tests/arm-cortex-m-bus-fault.robot` から 9 ケースを Rust に
移植しました。上流の RAM、ベクター、ハンドラー、故障する周辺機器のアドレスを
使用します。復帰トークンは ARMv7-M の Thread/MSP 基本フレーム `0xfffffff9`、
上流で使う NMI/HardFault アクティブビットは Cortex-M4F の内部例外状態を照合します。

- 保存では `CFSR.STKERR`、NMI による HardFault の保留、IRQ16 の pending/active、
  同優先度での BusFault 選択、HardFault を中断した NMI のロックアップと IT 状態を検査します。
- 復元では読み出しのみを故障させ、`CFSR.UNSTKERR`、NMI 復帰時のアクティブ状態、
  元のフレームを使うテールチェーンと FPCA クリアを検査します。
- 遅延 FP 保存では `FPCAR=0x100020`、`HFRDY`、`LSPACT`、`CFSR.LSPERR`、
  `HFSR.FORCED` と HardFault の保留を照合します。Thread と HardFault からの NMI で
  保留とロックアップをそれぞれ再現します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-stack-faults
mkdir -p "$work"
cargo test thumb::stack_fault_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos-fpu.log" 2>&1
    echo "$?" >"$work/freertos-fpu.status"
) &
```

`cargo test` は 90 件、フォールト ELF は 16 件が合格しました。FreeRTOS では
`FPU context OK: A`、`FPU context OK: B`、`IWDG reboot: SRAM retained` を確認しました。
公開の Cortex-M4 拡張フレーム ELF は上記の上流スモークの `fpu-v7m` 手順を単独で実行し、
`upstream-fpu.log` に `Test succeeded` を記録しました。実行上限での終了コード 2 と
UART 成功行を合わせて検証しています。TrustZone の追加スタック、共有のフォールト配送の
残件、一般の遅延 FP 準備ビットの組み合わせは今回の 9 ケースの外に残ります。

## STM32F407 USART の受信時刻

優先グループは固定上流 `tests/peripherals/STM32_UART.robot` の全 6 件です。
CPU を持たない上流フィクスチャと同様に、Rust のテストはアドレス `0x40000000` に
F4 UART を配置し、同じバスアクセスと 1 µs の時間刻みで検査します。16 MHz の入力と
`BRR=0x10` で、9 µs 後に RXNE が立たず、10 µs 後にデータを読めることを確認します。
パリティーを有効にしても 8 ビットのワードにパリティーを含めるため、フレーム長は
10 µs です。IDLE の検出・取り消し、BRR がゼロになったときのバイト順序、
UE/RE を無効にしたときの受信破棄も上流と同じ順序で検査します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-uart-timing
mkdir -p "$work"
cargo test stm32_uart::
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos-fpu" --fpu >"$work/freertos-fpu.log" 2>&1
    echo "$?" >"$work/freertos-fpu.status"
) &
```

FreeRTOS の期待値は `UART4 echo=X/Y/Z`、TIM2 のキュー通知、CAN ループバックと
IWDG 再起動です。`--fpu` では `FPU context OK: A` と `FPU context OK: B` も照合します。
送信タイミング、パリティーエラー、汎用端末テスターの未移植ケースは、この受信スイートの
合格件数には含めていません。

`cargo test` は 96 件、FreeRTOS の通常版と FPU 版はともに通過しました。
UART 入力変更に対する追加の短い回帰として、先の RTC 手順で取得した RIOT ELF に
同じ UART 計画を既定の 1 MIPS で適用し、4 回のアラームと最後の 3 秒の無通信を
確認しました。今回の短い回帰には 100 MIPS での再実行を含めていません。

```sh
rtc=target/stm32f407-timed-tests-rtc
target/release/penv run --script "$rtc/rtc.resc" --platform-root "$RENODE_ROOT" \
    --f4-repl-timing --uart-check "$rtc/rtc.tsv"
```

## Cortex-M4 Thumb DSP の GE フラグ

優先グループは固定上流 `tests/unit-tests/arm-ge-flag.robot` の 6 件です。
Cortex-M7 フィクスチャの各 10 命令を取り出し、同じオペランド・開始アドレス
`0x0` の Cortex-M4 ELF に組み立てます。最初の 10 命令が終わった直後の R4 を
追加した UART 出力で取得し、上流の期待値から生成した 4 バイトと照合します。

`sadd8`、`sadd16`、`ssub8`、`ssub16`、`usub8`、`usub16` の順に、
R4 が `0xff00ff00`、`0xffff0000`、`0x00ff00ff`、`0x0000ffff`、
`0xff0000ff`、`0x0000ffff` になります。上流の `usub8` というケース名の
実命令は `ssub8` です。上流の命令列を保持し、符号付きと符号なしの結果が
異なるオペランドで `USUB8` 自体も別の ELF で検査しました。この追加 ELF は
上流台帳の分母に含めていません。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-ge
mkdir -p "$work"
(
    tests/cortex-m4-ge.sh "$work/elf" >"$work/ge-elf.log" 2>&1
    echo "$?" >"$work/ge-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

`cargo test` 96 件、フォールト ELF 16 件、FreeRTOS の FPU 回帰が通過しました。
残件には CMSIS-DSP の公開 ELF と、飽和・半減など今回の命令群に含まれない
DSP 命令の検証があります。

## Cortex-M4 の IT ブロックと条件コード

固定上流 `tests/unit-tests/it-status.robot` から CPU の単一命令ステップだけで
再現できる 6 ケースを移植しました。Clang で上流アセンブリを Cortex-M4 に
組み立て、4 プログラムの命令バイトをテストの固定配列と照合しました。
上流と同じリセット／NMI アドレス `0x100`／`0x200`、初期 SP `0x800`、
ステップ数、レジスター条件を使用します。

AL の条件照会に対応し、例外保存と検査で xPSR の組み立てを共用します。
8 種類の条件照会と 4 組の xPSR、ITSTATE の `05 → 0a → 14 → 08 → 00`、
条件が偽の命令のスキップ、ブロック内部のフラグ更新と NMI 前後の ITSTATE を検査します。
保存・再読込の 1 件には永続スナップショット、残る 2 件にはブロック開始・終了フックと
ログ待機のモニター機能が必要です。この 3 件は未達です。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-it
mkdir -p "$work"
cargo test thumb::it_status_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

期待値は IT テスト 6 件と全 Rust テスト 102 件の合格、フォールト ELF 16 件の
判定行、FreeRTOS の `FPU context OK: A`、`FPU context OK: B` と
`IWDG reboot: SRAM retained` です。

## STM32F407 の共有 CRC エンジン

固定 Infrastructure の `src/Emulator/Main/Tests/UnitTests/CRCTests.cs` 全 15 件を
Rust に移植しました。STM32 の CRC DR が使う共通エンジンで、入力・出力反転、
初期値と出力 XOR、空入力と分割更新、CRC の各幅を照合します。上流の引数例外は
Rust の型付きエラーで照合します。移植は共有エンジンを対象とし、F407 上の
レジスターで設定できる幅と多項式は機種の定義に従います。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-crc
mkdir -p "$work"
cargo test crc
(
    tests/stm32f407-crc.sh "$work/elf" >"$work/crc-elf.log" 2>&1
    echo "$?" >"$work/crc-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

Discovery の未変更 kit REPL で実行する追加 ELF は、2 ワード更新、CR による
累積値のリセット、IDR の保持を確認すると、USART2 に
`STM32F407 CRC DR=49E3C2FB RESET=FFFFFFFF IDR=5A` を出力します。
この ELF は共有ケースの追加回帰であり、上流台帳の分母には含めていません。
DMA、GPIO、SystemBus など共有インフラのケースは別群として残っています。

## Cortex-M4F の精密 BusFault ロックアップと抑止

固定上流 `tests/unit-tests/arm-cortex-m-bus-fault.robot` の 3 件を、
同じ RAM、ベクター、故障アドレス `0x100000` と Thumb 命令で移植しました。
HardFault 中に IT ブロックのデータアクセスが失敗すると、CFSR と BFAR を記録し、
PC を `0xeffffffe` に固定します。ロックアップ中に 10 ステップを要求しても ITSTATE と
命令の完了数は変化せず、CPU リセットで CPU と NVIC のロックアップ状態が解除されます。
NMI は命令実行時ロックアップを中断でき、元の PC を `0xfd8` に保存します。

`CCR.BFHFNMIGN` は負の優先度のデータフォルトを抑止し、CFSR と BFAR の更新は行います。
Thread モードからの最初の失敗は HardFault に配送され、そのハンドラー内の同じ LDR
から継続して R1 が `0x22` になることを確認しました。Rust フィクスチャは例外配送と
命令の完了を別の呼び出しで観測します。追加検査では、同じ CCR 設定でも命令フェッチの
失敗がロックアップすることと BFAR が保持されることを確認します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-lockup
mkdir -p "$work"
cargo test thumb::stack_fault_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

期待結果は全 Rust テスト 120 件、フォールト ELF 16 件、FreeRTOS の FPU・IWDG 回帰の
合格です。Fault 後の NMI からロックアップ元に戻る経路や、TrustZone の診断別名を使う
上流ケースはこの 3 件の範囲外です。

## Cortex-M4F の BX・BLX と例外復帰

固定上流 `tests/unit-tests/arm-cortex-m-exception-return.robot` の非 TrustZone
適用ケース 4 件を移植しました。上流と同じ RAM、初期 SP、リセット／NMI ハンドラー
アドレス、Thumb 命令列とステップ順序を使用します。NMI の LR には ARMv7-M の
`0xfffffff9` が入るため、`BLX LR` 後の PC は `0xfffffff8` です。
`BX LR` は `0x20` に戻り、元のスタックを復元します。
Thread モードの `BX 0xffffffb8` では指定 PC を観測した後、次のフェッチで
`INVSTATE` とスタック上の障害 PC を検査します。`BX 0xfeffffff` は
通常の Thumb 分岐として `0xfefffffe` に到達します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-exc-return
mkdir -p "$work"
cargo test thumb::exception_return_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

期待結果は対象 4 件、全 Rust テスト 124 件、フォールト ELF 16 件、
FreeRTOS の FPU・IWDG 回帰の合格です。Armv8-M 固有の PMSAv8 ベクターケース、
`BXNS`／`BLXNS`、`FNC_RETURN` は台帳の対象外理由に従います。
汎用的な例外復帰トークンの検証や MPU の上流群は別の未達として残ります。

## Cortex-M4 の単一 CPU 排他アクセス

固定上流 `tests/unit-tests/arm-atomic.robot` の単一 CPU ケース 3 件を移植しました。
Cortex-R8 の A32 命令を Cortex-M4 の Thumb-2 で組み立て、同じ初期レジスター、
メモリーアドレス `0x1000`、データ値、操作順序で実行します。対象命令列の直後に
R2 とメモリーの値を UART へ出力し、32 ビット値を照合します。期待値は順に
`(0, 0x0a)`、`(1, 0x00)`、`(1, 0x0a)` です。

ワード排他命令の LR オペランドのデコード範囲を修正し、LDREX のワード境界を
バスアクセス前に検査します。LR の追加 ELF は上流台帳の分母に含めていません。
同一バス上の複数 CPU が必要な排他モニターとその保存・復元は台帳の対象外理由に従います。
GPIO の動的な切り離しには、周辺機器登録解除や外部コネクターのライフサイクルが
必要です。この共有群は未達に残ります。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
test "$(git -C "$RENODE_ROOT" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
work=target/stm32f407-exclusive
mkdir -p "$work"
(
    tests/cortex-m4-exclusive.sh "$work/elf" >"$work/exclusive-elf.log" 2>&1
    echo "$?" >"$work/exclusive-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

## 共有タイマーの周期境界

固定 Infrastructure の `src/Emulator/Main/Tests/UnitTests/LimitTimerTests.cs` 全 9 件を
移植しました。SysTick が使用する周期境界計算とイベント選択を、固定上流と同じ
周波数・上限・経過時間で検証します。上昇カウントは上限までの残距離から値を取得し、
方向切り替えでは切り替え時の値を保持します。1 MHz で 10 µs、10 MHz で 1 µs
進む各ケースで、期限到達 2 回と残り 2 クロックを確認します。
STM32 TIM は SysTick とクロック分数の繰り越しを共用し、TIM 固有の ARR と
更新・比較イベントの条件は各レジスターモデルに保持します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-limit-timer
mkdir -p "$work"
cargo test limit_timer
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

TIM2 の公開 ELF 回帰は `tests/stm32f407-upstream-smoke.sh` の TIM2 段を
個別に実行します。上流の SHA-1 に一致する上昇・下降ファームウェアを
各 100000 ステップ、未変更の CPU REPL で実行し、バナー、IRQ 有効化、開始、
3 回以上の周期コールバックの順序を照合します。ログは `tim2-up.log` と
`tim2-down.log` に保存しました。比較値変更と比較イベントを扱う共有
`ComparingTimerTests.cs` は別群に残っています。

## STM32 GPIO の静的接続

固定 Infrastructure の `src/Emulator/Main/Tests/UnitTests/GPIOTests.cs` 全 4 件を
Rust に移植しました。STM32 REPL のポート間配線も同じ接続登録を通ります。
受信側の識別子と入力ピン番号、未接続時の空リスト、5 入力の受信側のピン 2 を
確認しました。ピン 10 では上流と同じエラー文を検査します。モックオブジェクトの
参照同一性は、Rust のバス内で一意な受信側アドレスで照合します。

比較タイマー群は未達です。上流 `ComparingTimer` は上限超過の比較値を拒否し、
比較値や現在値を書いたときに期限を再計算します。STM32 TIM の CCR には ARR より
大きな値も書けるため、そのレジスター規則を維持した上で共有比較タイマーを扱う必要が
あります。GPIO の動的な切り離しも別群に残ります。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-gpio-connections
mkdir -p "$work"
cargo test renode_gpio
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

配線の回帰には `tests/stm32f407-upstream-smoke.sh` の Quadrature Decoder 段を
使います。公開 ELF の SHA-1 を照合し、上流 REPL に PD12→PA6 と PD13→PA7 を
追加して 2100000 ステップ実行します。期待値は最初の 2 出力が 10±1 度と 20±1 度です。
この実行の UART と判定結果は `qdec.uart` と `qdec.log` にあります。

## Cortex-M4 の SystemBus 未登録アクセスとタグ

固定 Infrastructure の `SystemBusTests.cs` から、未登録アドレスと範囲タグに関する
8 件を移植しました。REPL の `sysbus: init:` で `UnhandledAccessBehaviour Report`、
`DoNotReport`、`ThrowException`、`ThrowExceptionIfNotTagged` を選べます。
上流の既定モードを検証するケースでは `Report` を明示します。既存プラットフォーム用に
設定省略時の Penv のアクセス規則を保持しています。

`Tag` の `throwException` はログ抑止時にも有効です。子タグで範囲を分割しても親タグの
両側に値と例外指定が残ります。上流の `AddressError` は Cortex-M の
`BusError::Unmapped` として検査します。その他のログモード・タグオプションは残件です。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-unhandled-bus
mkdir -p "$work"
cargo test renode_sysbus
(
    tests/cortex-m4-sysbus.sh "$work/elf" >"$work/sysbus-elf.log" 2>&1
    echo "$?" >"$work/sysbus-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

追加 ELF は REPL を読み、未登録アドレスの 0、子タグの `0x42`、親タグで発生した
HardFault の `CFSR=0x8200` と `BFAR=0x1000` を検査して UART に `OK` を出します。
この ELF は台帳の分母に加えていません。

## SystemBus のマップ済みメモリー区間

固定 Infrastructure の `SystemBusTests.cs` からメモリー転送の 6 構成を移植しました。
2 アドレスに登録した 16 バイトの同一 RAM を使い、区間内、片端、両端、未登録区間への
書き込みを検査します。2 つの独立した RAM を使う `ShouldReadBytes` の
`true; true` は、350 バイトのうち登録区間だけに値が残り、隙間が 0 になることを
確認します。バイト周辺機器を含む残り 3 構成の結果は次節に記載します。

REPL は同一メモリーへの複数の `sysbus` 登録を受け付けます。`Report` または
`DoNotReport` を明示したスクリプトの `LoadBinary` では、対象データのうち
登録済みの RAM とバイト周辺機器へ書き込みます。追加 ELF は、ロード開始前半の
未登録区間が 0、後半の 4 バイトがエイリアスと主アドレスの双方で
`0x07060504` になることを検査し、UART に `OK` を出します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-mapped-bytes
mkdir -p "$work"
cargo test renode_mapped_bytes
(
    tests/cortex-m4-mapped-bytes.sh "$work/elf" >"$work/mapped-elf.log" 2>&1
    echo "$?" >"$work/mapped-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

## SystemBus の RAM・バイト周辺機器混在転送

固定 Infrastructure の `SystemBusTests.cs` の `ShouldReadBytes` で残っていた
`true; false`、`false; true`、`false; false` の 3 構成を移植しました。
C# のモックはバイト単位でアクセスする `Mocks.BytePeripheral` に対応させています。
バスは転送範囲を登録区間ごとに処理し、RAM へは区間転送し、バイト周辺機器へは
個別のバイト読み書きを行います。上流と同じ 50・200 番地の 100 バイト区間に対し、
350 バイトを入力し、全バイトを検査します。読出しの隙間は 0 です。
隣接領域を横断する検索は次節に記載します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-mixed-bytes
mkdir -p "$work"
cargo test renode_mapped_bytes
(
    tests/cortex-m4-mapped-bytes.sh "$work/elf" >"$work/mixed-elf.log" 2>&1
    echo "$?" >"$work/mixed-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

ELF スクリプトでは、既存のメモリーエイリアスに加え、RAM とバイト周辺機器の
混在 REPL を `LoadBinary` で初期化します。ELF が `LDRB` で 350 バイトすべてを
検査し、UART に `OK` を出力します。台帳は Rust の 3 構成を合格として数えます。

## SystemBus のバイトパターン検索

固定 Infrastructure `SystemBusTests.cs` の `ShouldFindBytePattern` を移植しました。
隣接する 1 MiB の RAM 2 区間に `00 01 02 03 04 05 06 07` を配置し、
アドレス `0x00000000` と `0x000ffffc` の 2 件だけが見つかることを確認します。
後者は RAM 登録境界と検索バッファー境界をまたぎます。未登録区間、バイト周辺機器、
フォールト注入用周辺機器は検索から除きます。エイリアスを含むアドレス順で結果を返します。

`penv run --script PATH --find-bytes HEX` はイメージをロードしてから検索し、
マッチしたアドレスを標準出力に 1 行ずつ出します。上流ケースの既定検索範囲に
対応します。検索開始・終了アドレスを指定する上流 API は別件に残ります。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-find-bytes
mkdir -p "$work"
cargo test renode_find_byte_pattern
(
    tests/cortex-m4-find-bytes.sh "$work/cli" >"$work/find-cli.log" 2>&1
    echo "$?" >"$work/find-cli.status"
    tests/cortex-m4-mapped-bytes.sh "$work/elf" >"$work/mixed-elf.log" 2>&1
    echo "$?" >"$work/mixed-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

### SystemBus の周辺機器例外

固定 Infrastructure の `ShouldPropagateBusAccessExceptionFromPeripheral` を移植しました。
上流と同じくベース `0x1000`、サイズ `0x100` の周辺機器に対してオフセット 0 の
32 ビット読み出しを行い、周辺機器が発生させた `CommandError` と同じインスタンスが
呼び出し元に戻ることを `Arc::ptr_eq` で確認します。補足の Cortex-M4 ELF は
`CFSR=0x8200` と `BFAR=0x1000` を確認し、UART に `OK` を出します。

上記の未登録アクセス検証手順で `work=target/stm32f407-command-error` に設定すると
再現できます。フォールト検証の実行ログは `faults.log`、ほかのログは手順と同じ名前で、
各終了状態は対応する `.status` に記録しています。

### SystemBus の 32 ビット周辺機器登録

固定 Infrastructure の `ShouldFindAfterRegistration` と
`ShouldFindAfterManyRegistrations` を Cortex-M4 ELF に移植しました。
`Mocks.DoubleWordPeripheral` を既存の固定値 MMIO と重複検査に接続し、
上流と同じ十進アドレス、登録サイズ、読み出し順序、応答値を使います。
各 ELF は全比較が一致したときだけ UART に `K` を出し、スクリプトが完全一致を検査します。
上流で `Ignore` 指定の登録解除ケースは、動的な登録解除と上流の乱数フィクスチャが必要なため未達です。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-doubleword-registration
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run sysbus-elf tests/cortex-m4-sysbus.sh "$work/elf"
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
) &
```

`sysbus-elf.log` には単独登録と複数登録の `OK` が出ます。各 `.status` の期待値は 0 です。

### 上流プラットフォーム定義の読み込み

`platform_descriptions.robot` から Discovery の独立した REPL 3 パラメーターと、
文字列読み込み 3 件を移植しました。各入力は独立プロセスで、通常の起動と共通の
REPL ローダーに渡します。プロセス終了が上流の `Reset Emulation` に相当します。
空文字列の成功と、不正な `using` および抽象型を指定した場合の終了コード 1 と
診断文字列を検査します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-platform-check
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run build cargo build --release
    run platform tests/stm32f407-platform-check.sh
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
) &
```

`platform.log` の期待値は REPL 3 件のロード成功、空文字列の成功、指定の診断 2 件です。
各 `.status` の期待値は 0 です。CAN の上流で無効化されたループバックケースは、
初期化モード外で CAN_BTR を書いて読み戻す期待値が同スイートの `InitTest` と矛盾し、
FINIT も解除しないため、元の条件を台帳に記録して未達に残しました。

### REPL の using エラー

固定上流の `UsingTests.cs` から、同一ソース内の重複、間接循環、直接循環、
存在しないファイルの 4 件を移植しました。上流のテスト用リゾルバーが作る
A と B を一時ファイルで再現し、元のソース文字列を `platform check --string`
に渡します。終了コード 1 に加え、上流と同じエラー種別
`DuplicateUsing`、`RecurringUsing`、`UsingFileNotFound` を検査します。

上記の REPL 読み込み手順で `work=target/stm32f407-repl-using` に設定し、
ビルド後に `run using tests/stm32f407-repl-using.sh` を追加すると再現できます。
`using.log` の期待値は重複 1 件、循環 2 件、欠落 1 件の診断です。
参照の可視性と宣言前の属性上書きは未達です。
初回の `cargo test` は循環エラーの従来文言を要求する CLI テストで失敗しました。
循環診断に従来文言を残して再実行した結果は `cargo-test.log`、初回結果は
`cargo-test-initial.log` に保存しています。

### REPL の Unix パス解決

固定上流の `UsingTests.cs` から、絶対パス、同一ディレクトリー、親ディレクトリー、
バックスラッシュをファイル名の文字として扱う 4 件を移植しました。
通常の REPL ローダーが使うリゾルバーに元と同じ引数・空の検索パスを渡し、
返るパスを完全一致で検査します。明示的なパスは字句的に解決します。
修飾のない名前の検索では、既存の同階層優先を保持します。

Windows 専用の 3 件は、上流の `RuntimeInfo.IsWindows()` ガードとテストが要求する
ドライブ・パス区切りを根拠に、Linux 対象の分母から外しました。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-using-paths
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
    run using tests/stm32f407-repl-using.sh
    run platform tests/stm32f407-platform-check.sh
) &
```

`cargo-test.log` に `renode_using_` で始まるケース 4 件と全体の合格が出ます。
各 `.status` の期待値は 0 です。

### REPL のコメントと文字列

固定上流の `PreLexerTests.cs` から 7 件を移植しました。通常の REPL ローダーが使う
コメント処理に元の入力を渡し、行コメント 4 件の出力を完全一致で検査します。
未終端文字列とブロックコメントの配置違反 2 件は構文エラーを検査します。
インデントから補う波括弧・セミコロンを含む全行変換とインデント診断は未達です。

直前の再現手順で `work=target/stm32f407-repl-comments` に変更すると、
関連回帰をまとめて実行できます。絞った検証は `cargo test renode_prelexer` です。
`cargo-test.log` の期待値は移植した 7 件の合格、各 `.status` の期待値は 0 です。
初回のコンパイルで検出されたファイル名の借用寿命は、重複検査がファイル名を所有するよう修正しました。
初回ログは `cargo-test-initial.log` に保存しています。

### REPL のインデント変換

固定上流の `PreLexerTests.cs` の残り 14 件を移植しました。
`penv platform prelex` は標準入力から REPL を読み込み、通常のローダーと共通の
コメント処理を通した後、インデントから波括弧・セミコロンを補ったテキストを返します。
空行、明示的な波括弧、文字列中の波括弧、複数行コメントの配置も検査しました。
先頭行の不正なインデントは `WrongIndent` を返します。

直前の再現手順で `work=target/stm32f407-repl-prelex` を使い、最初に
`run focused cargo test renode_prelexer` を実行します。`focused.log` の期待値は
このスイートの 21 件合格です。CLI 単体では
`printf 'parent\n    child' | target/release/penv platform prelex`
を実行すると、`parent{` に続いて `    child}` が出ます。
通常の REPL ローダーは既存の構文解析を使います。他の REPL パーサースイートは引き続き対象です。

### REPL の sysbus 範囲と十六進数

固定上流の `ParserPrimitivesTest.cs` から 8 パラメーターを移植しました。
sysbus の範囲登録で `<開始, 終端>` と `<開始, +長さ>` を受け付け、
十進数と十六進数の組み合わせを解釈します。逆順の境界は拒否します。
パーサーは 64 ビットの値を読み取り、STM32F407 の登録時には
開始と長さを既存の 32 ビット型に変換します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-repl-ranges
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run focused cargo test renode_ranges
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
    run platform tests/stm32f407-platform-check.sh
) &
```

`focused.log` は 3 テスト関数で上流の 8 入力を検証します。
各 `.status` の期待値は 0 です。符号付き・小数の汎用トークン、IRQ リスト、
引用文字列のケースは未達です。

### REPL の IRQ 範囲とソースリスト

固定上流の `ParserPrimitivesTest.cs` から IRQ 構文 3 件を移植しました。
昇順・降順の範囲を指定順に展開し、番号と名前付きソースのリストを読み取ります。
既存の番号配線はこのパーサーを使います。名前付きソースを含む任意のリストの
デバイス別配線は今後の対応範囲です。

直前の範囲検証手順で `work=target/stm32f407-repl-irq` とし、絞った検証を
`run focused cargo test renode_irq_primitive` に置き換えると再現できます。
`focused.log` の期待値は 3 件合格で、各 `.status` の期待値は 0 です。

### REPL の数値トークンと単行引用文字列

固定上流の `ParserPrimitivesTest.cs` の残り 14 入力を移植しました。
符号付き十六進数、整数、小数のトークン出力、無効な数値の拒否、
および空白やセミコロンを含む引用文字列の復号を検証します。
数値トークン処理は既存のアドレス・長さの変換で使い、登録時には符号なし整数に
変換します。単行引用文字列は通常の `using` で使います。

直前の範囲検証手順で `work=target/stm32f407-repl-scalars` とし、最初に
`run focused cargo test renode_scalar`、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
`focused.log` の期待値は 4 テスト関数に含まれる上流 14 入力の合格です。
各 `.status` の期待値は 0 です。型付きプロパティへの汎用バインドと
複数行引用文字列は別の上流ケースとして未達です。

### REPL の複数行引用区切り

`platform prelex` が三重引用符を含む物理行をまとめ、エスケープされた区切りを
区別します。固定上流 `MergeAndCreationTests.cs` の元の入力 4 件で、
`SyntaxError` と `At 11:17:`、`At 12:9:`、`At 3:37:`、`At 3:17:` を検査します。
オブジェクト生成時の文字列の値、引用符の後の構文、`using` の構文は残課題です。

直前の範囲検証手順で `work=target/stm32f407-repl-multiline` に設定し、
`run focused cargo test renode_multiline` に置き換えます。
`focused.log` の期待値は 4 件合格で、各 `.status` の期待値は 0 です。

### REPL ローダーでの複数行文字列エラー

通常の REPL ローダーで、固定上流の複数行文字列エラーを 2 件移植しました。
`using` に複数行の引用を渡すと `SyntaxError` になり、プロパティの閉じ区切り後の
余分な文字列には `SyntaxError` と `At 4:23:` を返します。
成功系でのプレースホルダーへの値のバインドと、開始行の文字列内部にあるコメント記号の
扱いは未達です。

直前の手順で `work=target/stm32f407-multiline-loader` に設定して、
`run focused cargo test renode_multiline_loader` を実行します。
`focused.log` の期待値は 2 件合格です。加えて
`run using tests/stm32f407-repl-using.sh` を実行します。
各 `.status` の期待値は 0 です。

### REPL の複数行文字列の値

固定上流の成功ケース 7 件を移植しました。通常の REPL ローダーで引用内容を保護し、
複数行の値を読み取ります。実際の Python 周辺機器のスクリプトも共通の復号を経由します。
上流の `MockCPU` 相当のテスト用フィクスチャは、通常のパースとマージで得た
登録名・プロパティを使い、`sysbus.cpu` と `sysbus.cpu2` の文字列を完全一致で検証します。
物理行末のエスケープ 2 件の検証結果は次節に記録します。

範囲検証手順で `work=target/stm32f407-multiline-binding` とし、
`run focused cargo test renode_multiline` を最初に実行します。
`focused.log` の期待値は関連する 13 件合格、各 `.status` の期待値は 0 です。

### REPL の行末にあるエスケープ済み区切り

固定上流の残り 2 件を元の REPL と期待値で移植しました。上流の C# 入力は
逐語的文字列なのでバックスラッシュを含みます。期待値の通常の C# 文字列では
同じ表記がアポストロフィに復号されます。既存のローダーが三重引用符と改行を保持し、
復号後の値が一致することを検証しました。

直前の手順で `work=target/stm32f407-multiline-escaped-eol` として
`run focused cargo test renode_multiline` を実行します。`focused.log` の期待値は
関連 15 件の合格です。上流で無効化された引用ケースと他の生成ケースは未達です。

### REPL のプロパティ上書き順序

固定上流の単一ファイルのプロパティ更新 1 件と `using` の順序 5 件を移植しました。
通常のローダーは全入力から宣言先を把握し、取り込んだファイルとルートのブロックを
読み込み順に適用します。後のブロックが同名のスカラー値を上書きします。
上流の入力と同じインポートグラフを一時ファイルで再現し、最終値と保持される
別プロパティを検査します。登録先の差し替えとインポートをまたぐ参照の可視性は未達です。

既存の範囲検証手順で `work=target/stm32f407-repl-overrides` とし、最初に
`run focused cargo test renode_override`、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
期待値は絞った検証 6 件合格と各 `.status` の 0 です。

### REPL の登録先変更と解除

固定上流の登録先変更と `@none` による解除の 2 件を移植しました。
宣言のマージは後続ブロックの登録先を適用します。型付き `BusRangeRegistration` の
インライン波括弧も登録式として処理します。
上流の模擬機器に相当するテスト用フィクスチャで登録先の名前を照合し、
補足テストでは実際の SystemBus に移動先のみがマップされることを読み出しで確認します。
一般の型付き登録や機器生成時の依存順序は残課題です。

範囲検証手順で `work=target/stm32f407-repl-registration` とし、
最初に `run focused cargo test renode_registration` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。
初回は型付き範囲の波括弧が機器のプロパティと解釈されました。修正前のログは
`focused-initial.log` に保存しています。

### REPL の登録別名

固定上流の別名登録 1 件と不正な登録 2 件を移植しました。宣言の識別子を配線に、
別名を登録名と Peripheral Export に使います。登録なしと `@ none` を伴う別名は
それぞれ `AliasWithoutRegistration` と `AliasWithNoneRegistration` を返します。
名前付き親の依存順序と、インポートのスコープにまたがる参照は残課題です。

範囲検証手順で `work=target/stm32f407-repl-alias` とし、
最初に `run focused cargo test renode_alias` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。

### REPL の宣言内属性検証

固定上流の 3 件を移植しました。同一宣言内の属性と `init` の重複、
および既存の `sysbus` に対するコンストラクター属性を、機器生成前に拒否します。
元の REPL 入力で上流と同じエラー種別を検査します。
型付きコンストラクターの選択と登録先の曖昧性検証は未達です。

範囲検証手順で `work=target/stm32f407-repl-attributes` とし、最初に
`run focused cargo test renode_attribute` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。

### REPL の登録点と IRQ の参照検証

固定上流の未宣言登録点と未宣言 IRQ 宛先の 2 件を移植しました。
宣言をマージした後、機器を生成する前に参照先を調べます。
元の入力で `MissingReference` と `IrqDestinationDoesNotExist` を検査します。
一般プロパティの参照型や IRQ 受信能力の検証は残課題です。

範囲検証手順で `work=target/stm32f407-repl-references` とし、最初に
`run focused cargo test renode_reference` を実行します。
期待値は絞った検証 2 件合格と各 `.status` の 0 です。

### REPL の IRQ 配線検証

固定上流の 3 件を移植しました。単一宛先の明示配線で入出力数を比較し、
同じ宣言内で番号付きまたは省略された既定の出力が重複する場合にエラーを返します。
元の入力で `WrongIrqArity` と `IrqSourceUsedMoreThanOnce` を検査します。
モデルごとの既定出力と名前付き出力の同一性や、複数宛先を含む個数検証は残課題です。

範囲検証手順で `work=target/stm32f407-repl-irq-validation` とし、最初に
`run focused cargo test renode_irq_validation` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。

### REPL のモデル別 IRQ 出力検証

固定上流の 5 件を移植しました。既知のモデルでは GPIO 出力名と既定出力の定義を
使い、未知の名前、番号非対応、既定出力の欠落・曖昧性、および同一出力の重複指定を
上流と同じ種別で報告します。固定フィクスチャに加えて STM32 UART にも適用します。
その他のモデルへの適用、受信能力、複数宛先の検証は残課題です。

範囲検証手順で `work=target/stm32f407-repl-irq-models` とし、最初に
`run focused cargo test renode_irq_` を実行します。
期待値は新規 5 件を含む IRQ テストの合格と各 `.status` の 0 です。
初回の全体テストで発見した診断順の不一致を修正し、未宣言宛先の検証を出力名の
検証より先に適用しました。初回ログは `cargo-test-initial.log` に保存しています。

### REPL の IRQ 受信能力と番号付き出力

固定上流の 2 件を移植しました。受信できないと分かっているモデルへの配線は
`IrqDestinationIsNotIrqReceiver`、既知の出力範囲を超える番号は
`IrqSourcePinDoesNotExist` を返します。番号の検査は固定フィクスチャと
STM32 CAN に適用します。未知モデルの能力と複数宛先の検証は残課題です。

直前の手順で `work=target/stm32f407-repl-irq-capabilities` として再現します。
期待値は追加 2 件を含む IRQ テストの合格と各 `.status` の 0 です。

### REPL の型宣言と登録の順序

固定上流のエラー 2 件を等価移植し、成功 2 件の宣言マージを検証しました。機器の型を登録の前後どちらで記述しても
宣言をマージし、元の入力で範囲を検査します。同じ識別子への型の再指定は
`VariableAlreadyDeclared`、内容のない宣言は `EmptyEntry` を返します。
成功 2 件は上流の EmptyPeripheral の生成まで通していないため部分移植です。
汎用の型解決と名前付き登録先の生成順序は残課題です。

範囲検証手順で `work=target/stm32f407-repl-declarations` とし、最初に
`run focused cargo test renode_declaration` を実行します。
期待値は絞った検証 4 件合格と各 `.status` の 0 です。

### REPL の EmptyPeripheral 生成

固定上流の引数なし `EmptyPeripheral` をローダーに追加しました。
周辺機器だけのフィクスチャは `platform check` で構築し、登録された範囲では
既存の固定値 MMIO を使います。部分移植だった型宣言・登録順 2 件と、
未登録の単一機器 1 件を元の入力で実行しました。登録したフィクスチャは
範囲内の読み出しが 0、範囲外がアクセスエラーであることも検査します。
引数付きコンストラクター、状態付きプロパティ、I²C 接続は残課題です。

範囲検証手順で `work=target/stm32f407-empty-peripheral` とし、
最初に `run focused cargo test renode_declaration` を実行します。
期待値は編入した 3 件を含む宣言テスト 5 件の合格と各 `.status` の 0 です。

### REPL の EmptyPeripheral コンストラクター診断

固定上流の 3 件を移植しました。インライン型の欠落を先に検査し、
対応する引数がない場合と整数が二つのコンストラクター候補に適合する場合を区別します。
元の入力で `TypeNotResolved`、`NoCtor`、`AmbiguousCtor` を検査します。
引数付きコンストラクターの成功系と、他の型の一般的な選択は残課題です。

範囲検証手順で `work=target/stm32f407-empty-constructors` とし、最初に
`run focused cargo test renode_empty_ctor` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。

### REPL の型と生成可否の診断

固定上流の 3 件を移植しました。型が解決できない宣言、バス登録インターフェイスを
持たない外部オブジェクト、公開コンストラクターがないモデルを区別し、
元の入力で `TypeNotResolved`、`NoUsableRegisterInterface`、`NoCtor` を検査します。
名前付き登録先が周辺機器として使えるかを検証するケースには、登録先の解決と
インターフェイスの検証が必要です。

範囲検証手順で `work=target/stm32f407-repl-creation-types` とし、最初に
`run focused cargo test renode_creation_type` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。

### REPL の登録点と登録先の型

固定上流の 3 件を移植しました。登録点に CPU 参照またはインライン CPU が
指定されると `TypeMismatch`、登録先が既知の非周辺機器の場合は `CastException`
を返します。インライン登録点の波括弧は登録式として扱います。
任意の登録点型の構築や、登録可能なインターフェイスの一般的な選択は残課題です。

範囲検証手順で `work=target/stm32f407-repl-registration-types` とし、最初に
`run focused cargo test renode_registration` を実行します。
期待値は新規 3 件を含む絞った検証 6 件合格と各 `.status` の 0 です。

### REPL の登録点候補の診断

固定上流の 4 件を移植しました。文字列を sysbus の登録点として渡した場合は
適合するコンストラクターがないことを報告します。上流の `AmbiguousRegister`
フィクスチャでは、登録点コンストラクター、登録点インターフェイス、
登録対象インターフェイスの曖昧性を区別します。
任意のモデルの登録候補探索と名前付き親への正常登録は残課題です。

範囲検証手順で `work=target/stm32f407-repl-regpoint-resolution` とし、最初に
`run focused cargo test renode_registration` を実行します。
期待値は新規 4 件を含む絞った検証 10 件合格と各 `.status` の 0 です。

### REPL の MockCPU プロパティ検証

固定上流の 8 件を移植しました。上流の `MockCPU` 型情報に基づき、文字列・列挙値の型、
読み取り専用・未定義プロパティ、CPU 参照とインライン型の解決を生成前に検証します。
ネストしたインラインオブジェクトでは個別のプロパティスコープの保持が必要です。
インポートの可視性と他のモデルへの一般化も残課題です。

範囲検証手順で `work=target/stm32f407-repl-properties` とし、最初に
`run focused cargo test renode_property_` を実行します。
期待値は絞った検証 8 件合格と各 `.status` の 0 です。

### REPL のネストしたインラインオブジェクト

固定上流の 3 件を移植しました。インデントで子の属性をまとめ、
子オブジェクトごとにプロパティまたはコンストラクターを検証します。
元の入力で `TypeMismatch`、`NoCtor`、`AmbiguousCtor` を検査しました。
対象は固定フィクスチャのモデルで、任意モデルのインライン生成と波括弧形式は残課題です。

範囲検証手順で `work=target/stm32f407-repl-nested-inline` とし、最初に
`run focused cargo test renode_nested_inline` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。

### REPL の using をまたぐ CPU 参照

固定上流の 4 件を移植しました。元の REPL と取り込みグラフで、直接参照、ネストした
取り込み、取り込み元への逆方向参照が成功し、別ソースの `local` 宣言への参照は
`MissingReference` になることを確認します。未登録の上流 `MockCPU` は
`platform check` で型と属性を検証します。実行 CPU としての機能、同名のローカル宣言を
別々のスコープに置くケース、他の種類の参照への適用は残課題です。

範囲検証手順で `work=target/stm32f407-repl-reference-imports` とし、最初に
`run focused cargo test renode_using_reference`、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
期待値は絞った検証 4 件合格と各 `.status` の 0 です。

### REPL の using 構文

固定上流のパーサーから空入力 1 件と `using` 5 件を移植しました。
元の入力で引用パスの列を検査し、同じ入力を通常のローダーにも渡しました。
末尾セミコロンの有無と各行の組み合わせを検証し、補足テストでは
キーワードの後のタブも確認します。残る宣言・登録点の構文は未達です。

範囲検証手順で `work=target/stm32f407-repl-using-parser` とし、最初に
`run focused cargo test renode_using_`、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
絞った検証は上流由来 6 件と補足ケースを既存の `using` テストと同じバイナリで実行し、
`focused.log` にケース別の結果を出します。各 `.status` の期待値は 0 です。

### REPL の登録点構文

固定上流の宣言・登録点構文 12 件をまとめて移植しました。単一の波括弧登録を
通常の登録式に変換し、複数の登録先では各項目を識別子から始めることを検証します。
入れ子の登録式のセミコロンと、登録一覧を区切るセミコロンを区別します。
一般の機器で複数箇所を登録する機能と、属性構文の残りは未達です。

範囲検証手順で `work=target/stm32f407-repl-registration-syntax` とし、
`cargo test`、FreeRTOS ELF、上流 REPL 読み込みを順に実行します。
`cargo-test.log` の `renode_registration_syntax_` 12 行がケース別の合否です。

### REPL の属性ブロック構文

固定上流の 18 件をまとめて移植しました。一行・複数行の波括弧属性を読み込み、
空の属性ブロックと不正な空ステートメントを区別します。列挙値の型名指定と省略、
不正な識別子、セミコロンの位置も元の入力で検査します。
インラインオブジェクトの任意の入れ子と、詳細な IRQ 構文は残課題です。

範囲検証手順で `work=target/stm32f407-repl-attribute-syntax` とし、
`cargo test`、FreeRTOS ELF、上流 REPL 読み込みを順に実行します。
`cargo-test.log` の `renode_attribute_syntax_` 18 行がケース別結果です。

### REPL の多宛先 IRQ 構文

固定上流の IRQ 構文 5 件をまとめて移植しました。番号・名前・省略の各ソース、
出力先の分岐、範囲を含むリスト、ローカル受信機番号を元の入力と照合します。
明示ソースの本数は各宛先に対して検証します。任意機器への多宛先配線の適用と、
残る初期化・型付き値の構文は未達です。

範囲検証手順で `work=target/stm32f407-repl-irq-syntax` とし、
`cargo test`、FreeRTOS ELF、上流 REPL 読み込みを順に実行します。
`cargo-test.log` の `renode_irq_syntax_` 5 行がケース別結果です。

### REPL の init コマンド構文

固定上流の 3 件をまとめて移植しました。インデントされたコマンドを一つの
`init` 属性にまとめ、引用文字列内のセミコロンを保持します。
同じコマンド解析を sysbus の初期化に使います。元の入力から得られるコマンド列を
完全一致で検査しました。一般のモニターコマンド実行と型付き値の構文は残課題です。

範囲検証手順で `work=target/stm32f407-repl-init-syntax` を使い、最後に
`run sysbus tests/cortex-m4-sysbus.sh "$work/sysbus"` を加えます。
`cargo-test.log` の `renode_init_syntax_` 3 行がケース別結果です。

### REPL の型付きスカラー属性

固定上流の 3 件をまとめて移植しました。文字列、数値、参照、範囲の型と値、
真偽値として認識する大文字・小文字の組み合わせ、引用文字列のバックスラッシュを
元の入力で検査します。型分類は `MockCPU` の属性検証でも使います。
任意のデバイスへの型付き値のバインドと、リスト・辞書の構文は未達です。

範囲検証手順で `work=target/stm32f407-repl-typed-scalars` を使います。
`cargo-test.log` の `renode_typed_scalar_` 3 行がケース別結果です。

### REPL のリスト値

固定上流のリスト 5 件をまとめて移植しました。空、末尾コンマ、入れ子、
混在するスカラー値、インラインオブジェクトを同じ入力で検証します。
区切りの解析では内側の括弧と引用文字列を区別します。
辞書はキーと値を分け、不正な区切りの位置を診断する処理が必要です。
任意のモデルへのリストの適用も残課題です。

範囲検証手順で `work=target/stm32f407-repl-list-values` を使います。
`cargo-test.log` の `renode_list_value_` 5 行がケース別結果です。

### REPL の辞書値

固定上流の辞書 7 件をまとめて移植しました。空辞書、キーと値、末尾セミコロンを
検証し、閉じ波括弧・区切り・コロンの欠落と不正なコンマには
上流のテストが要求する診断文言を確認します。任意の機器への辞書値の適用は残課題です。

範囲検証手順で `work=target/stm32f407-repl-dictionaries` を使います。
`cargo-test.log` の `renode_dict_` 7 行がケース別結果です。

### REPL 記述全体の構文

固定上流の `ParserTests.cs` に残る 7 件をまとめて移植しました。
インデントで記述したオブジェクト値、複数宣言、`local`、取り込みと宣言の組み合わせ、
複合例、LF と CRLF の混在を元の入力で検査します。
これは構文スイートの検証です。モデル生成・参照・登録の残件は各上流ケースに残しています。

範囲検証手順で `work=target/stm32f407-repl-description` を使い、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
`cargo-test.log` の `renode_description_` 7 行がケース別結果です。

### REPL の予約語 empty

固定上流の 4 件をまとめて移植しました。型名、登録先、属性名、`using` のパスに
`empty` を使う元の入力を、通常のローダーが `SyntaxError` で拒否します。
`empty` を値として使った場合の型別の既定値は残課題です。

範囲検証手順で `work=target/stm32f407-repl-empty-keyword` を使い、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
`cargo-test.log` の `renode_empty_keyword_` 4 行がケース別結果です。

### REPL の init 置換と追記

同じ周辺機器の後続 `init` でコマンドを置き換え、`init add` で追記します。
固定上流の 2 件は元の入力とコマンド列が一致しました。上流の模擬スクリプトハンドラーへの
実行通知は未移植のため、台帳では部分移植です。補足テストは実際の SystemBus で
置換・追記後のタグを読み出します。`empty` の値ケースには型付きモデルの
生成・束縛と既定値の観測が必要です。

範囲検証手順で `work=target/stm32f407-repl-init-merge` を使い、最後に
`run sysbus tests/cortex-m4-sysbus.sh "$work/sysbus"` を加えます。
`cargo-test.log` の `renode_init_merge_` 3 行がケース別結果です。

### REPL の init ハンドラーへの通知

通常の機器生成経路から初期化コマンドをハンドラーへ渡します。
上流の 3 件と同じ入力に対し、置換・追記したリストと sysbus の `WriteByte 0 1` が
上流と同様にテスト用ハンドラーへ一度だけ届くことを検査しました。
既定の sysbus ハンドラーは既存のコマンドを処理します。一般のモニター実行、
`WriteByte` の既定ハンドラーでの実行、スクリプト検証・リセットは残課題です。

直前の再現手順で `work=target/stm32f407-repl-init-dispatch` を使います。
`cargo-test.log` の `renode_init_merge_` 4 行が補足テストを含むケース別結果です。

### REPL の reset 登録通知

固定上流の `reset` 置換と `reset add` 追記の 2 件を移植しました。
機器生成時にリセット登録フェーズとして処理し、元の入力で期待するコマンド列が
テスト用ハンドラーに一度届くことを確認しました。
既定ハンドラーは任意の周辺機器リセットスクリプトの登録・実行に未対応です。
既存の IWDG 再起動は FreeRTOS ELF で回帰検証します。

直前の再現手順で `work=target/stm32f407-repl-reset-dispatch` を使います。
`cargo-test.log` の `renode_reset_dispatch_` 2 行がケース別結果です。

### REPL の preinit 実行順

固定上流の 4 件を移植しました。`preinit` の置換と `preinit add` の追記を
機器生成前に適用し、sysbus を先頭にして宣言順にハンドラーへ通知します。
元の入力でコマンド列と通知順を検査しました。既定ハンドラーでの任意コマンド実行と、
ハンドラーのセクション検証エラーは残課題です。

直前の再現手順で `work=target/stm32f407-repl-preinit` を使います。
`cargo-test.log` の `renode_preinit_dispatch_` 4 行がケース別結果です。
初回に見つかった sysbus のコンストラクター属性との誤認を修正しました。
初回ログは `cargo-test-initial.log` に保存しています。

### REPL のスクリプト検証エラー

固定上流の `preinit` と `init` の 2 件を移植しました。取り込みと上書き後の
コマンド列を、機器生成や実行の前にハンドラーで検証します。テスト用ハンドラーの
拒否理由をフェーズ別の上流エラー種別とともに返し、実行に進まないことを確認します。
組み込みハンドラーの対応範囲は既存の sysbus コマンドです。一般のモニター実行と
周辺機器リセットスクリプトの実行は残課題です。

直前の再現手順で `work=target/stm32f407-repl-script-validation` を使います。
`cargo-test.log` の `renode_script_validation_` 2 行がケース別結果です。

### REPL のコンストラクター依存関係

固定上流の 4 件をまとめて移植しました。マージ後にコンストラクター参照を
解決し、ネストしたインラインオブジェクト、自己参照、取り込みをまたぐ循環を
`CreationOrderCycle` として拒否します。未宣言の引数参照は `MissingReference` です。
今回の依存関係の検査は固定上流フィクスチャのコンストラクターに適用します。
任意のモデルの依存順での生成や、生成時の例外伝搬は残課題です。

範囲検証手順で `work=target/stm32f407-repl-creation-cycles` を使い、最後に
`run using tests/stm32f407-repl-using.sh` を加えます。
`cargo-test.log` の `renode_dependency_` 4 行がケース別結果です。

### REPL の生成時例外

固定上流の 2 件を移植しました。`platform check` で未登録の
`MockPeripheralWithDependency` のコンストラクターを処理し、
トップレベルとインラインオブジェクトのどちらでも元の入力が
`ConstructionException` を返すことを確認します。
一般のモデル生成、プロパティ設定と登録時の例外は残課題です。

範囲検証手順で `work=target/stm32f407-repl-construction-errors` を使います。
`cargo-test.log` の `renode_construction_exception_` 2 行がケース別結果です。

### REPL の複数範囲登録と登録例外

固定上流の `EmptyPeripheral` 登録 3 件をまとめて検証しました。
複数行の登録リストに続く別名を取り込み、各範囲を実際の SystemBus に登録します。
重複範囲と同名登録は上流と同じエラー種別を返します。
正常系では別名と範囲順、各範囲の境界での読み出しを確認しました。実行時の
名前からの機器取得と登録点の列挙は未移植のため、この正常系は部分移植です。
別の親への登録と他のモデルの複数範囲も残課題です。

直前の再現手順で `work=target/stm32f407-repl-multi-registration` を使い、最後に
`run sysbus tests/cortex-m4-sysbus.sh "$work/sysbus"` を加えます。
`cargo-test.log` の `renode_peripheral_registration_` 3 行がケース別結果です。

### REPL の実行時登録範囲検索

固定上流の複数登録ケースを等価合格に更新しました。SystemBus の登録名で
`sysbus.alias` を検索し、同じ機器の二つの範囲を登録順に取得します。
元の宣言名が登録されていないことと、範囲の境界も検査します。
登録途中に重複が発生すると、この操作で追加した範囲を取り消します。
Peripheral Export は同じ登録情報を使います。他モデルへの一般化は残課題です。

直前の再現手順で `work=target/stm32f407-repl-registration-lookup` を使います。
`cargo-test.log` の `renode_peripheral_registration_` 4 行がケース別結果です。
初回のコンパイルエラーは出力の分岐が値を返すよう修正しました。
初回ログは `cargo-test-initial.log` に保存しています。

### REPL の名前付き親への登録

`platform check` が固定上流の登録先フィクスチャを SystemBus に配置し、
続いて子 CPU を登録します。先に子を宣言する場合、同じ子を二つの親から検索する場合、
未登録の親を拒否する場合の 3 件を移植しました。既存の登録先変更と解除の 2 件も
同じ SystemBus の検索に切り替えました。
この構成は上流の模擬 CPU の検証用です。既存の実行中マシンへの追加や一般モデルの
登録インターフェイスは残課題です。

直前の再現手順で `work=target/stm32f407-repl-named-registration` を使います。
`cargo-test.log` の `renode_named_registration_` 3 行が新規ケース別結果です。

### REPL の列挙型コンストラクター引数

固定上流の列挙値フィクスチャ 7 件をまとめて移植しました。型名付き・省略形・整数を
コンストラクター引数に変換し、SystemBus の登録名から値を確認します。
通常の列挙型では未定義の整数を拒否し、数値を任意に許可する列挙型では
基底型に収まる整数を受け付けます。両方で未知の列挙名は `NoCtor` です。
他モデルへの適用と既存マシン内の登録済み機器を変数として解決する処理は残課題です。

範囲検証手順で `work=target/stm32f407-repl-enum-fixture` を使います。
`cargo-test.log` の `renode_enum_fixture_` 7 行がケース別結果です。

### REPL の型宣言に先行する CPU 登録点

固定上流の 2 件を一括で検証しました。型宣言が後にある `MockCPU` を
`platform check` の SystemBus で登録名から検索できることと、不適合な範囲登録点が
`NoCtorForRegistrationPoint` になることを元の入力で確認しました。
実行中の CPU の追加や任意の登録点は残課題です。

範囲検証手順で `work=target/stm32f407-repl-previous-registration` を使い、
最後に `run sysbus tests/cortex-m4-sysbus.sh "$work/sysbus"` を追加します。
`cargo-test.log` の `renode_previous_registration_` 2 行がケース別結果です。

### REPL の empty によるスカラー既定値

固定上流の 3 件を移植しました。整数、文字列、真偽値のコンストラクターが
`empty` を受け取ったとき、登録名から取得した値がそれぞれ整数 0、null 文字列、
false であることを元の入力で検査します。対象は `platform check` の固定フィクスチャです。
列挙型、範囲、参照、コレクションの既定値は残課題です。

範囲検証手順で `work=target/stm32f407-repl-empty-scalars` を使います。
`cargo-test.log` の `renode_empty_scalar_` 3 行がケース別結果です。
初回は元の入力にある `sysbus` と範囲の間の追加空白で 3 件が失敗しました。
共通範囲パーサーを修正し、初回結果は `cargo-test-initial.log` に保存しています。

### REPL の empty による範囲・参照の既定値

固定上流の 3 件をまとめて移植しました。SystemBus の登録名から値を取得し、
`default(Range)` の始点と終点がともに 0、オブジェクトと周辺機器参照が
それぞれ null になることを元の入力で検査します。上流の範囲は終端を含むため、
始点と長さの表現では `(0, 1)` です。
非 null の任意オブジェクト・参照や、コレクションの束縛は残課題です。

範囲検証手順で `work=target/stm32f407-repl-empty-bindings` を使います。
`cargo-test.log` の `renode_empty_binding_` 3 行がケース別結果です。

### REPL のコレクション引数の束縛

固定上流 12 件を一括で移植しました。登録名を使い、整数リスト・配列、
文字列リスト、整数キーと文字列値の辞書を確認します。`empty` は null、
`[]` と `{}` は空のコレクションとして区別します。上書き後の値と、要素の
型不一致も元の入力で検査しました。CPU を要素またはキーにするケースは
登録オブジェクトへの参照を束縛する処理が必要です。

範囲検証手順で `work=target/stm32f407-repl-collection-fixtures` を使います。
`cargo-test.log` の `renode_collection_fixture_` 12 行がケース別結果です。

### REPL のコレクション内 CPU 参照

固定上流 5 件をまとめて移植しました。SystemBus の模擬 CPU を登録してから
コレクションを束縛し、先行・後続宣言の参照を解決します。リスト要素、辞書の値とキーが
登録された CPU と同じ参照を保持することを検査します。辞書キーの重複では
上流の例外種別と内側のメッセージを診断文字列として確認します。
この束縛は検証用の模擬 CPU が対象です。実行中マシンの任意機器参照は残課題です。

範囲検証手順で `work=target/stm32f407-repl-collection-references` を使います。
`cargo-test.log` の `renode_collection_reference_` 5 行がケース別結果です。

### REPL の MockCPU 列挙プロパティ既定値

固定上流の `none` と `empty` の 2 件を一括で移植しました。型が
`TwoStateEnum` のプロパティを登録済み CPU から取得し、両入力で `One` になることを
検査します。補足ケースで明示した `Two` も保持されることを確認します。
任意のモデルのプロパティ初期化や、コンストラクター引数への `none` は残課題です。

範囲検証手順で `work=target/stm32f407-repl-cpu-enum-default` を使います。
`cargo-test.log` の `renode_cpu_enum_default_` 3 行がケース別結果です。
初回は編集スクリプトが途中で停止したためコンパイルに失敗しました。
初回ログは `cargo-test-initial.log` に保存しています。

### REPL のコンストラクター引数の既定値

固定上流の 3 件を移植しました。上書きした `value: none` は
`EmptyPeripheral` の引数を取り消し、登録された機器のカウンターが 1 になります。
固定フィクスチャのマシン引数はローダーが渡し、同名に近い整数引数 `machine`
には `empty` を指定できます。自動で渡す `mach` の上書きは `NoCtor` です。
任意の型のコンストラクター選択と引数注入は残課題です。

範囲検証手順で `work=target/stm32f407-repl-constructor-defaults` を使い、
最後に `run sysbus tests/cortex-m4-sysbus.sh "$work/sysbus"` を加えます。
`cargo-test.log` の `renode_constructor_default_` 3 行がケース別結果です。
初回はモデル分岐を誤ったスコープに追加してコンパイルに失敗しました。
初回ログは `cargo-test-initial.log` に保存しています。

### REPL のインラインプロパティ設定

固定上流の 3 件を一括で移植しました。登録済み `MockCPU` の二段の
`OtherCpu` をたどって `Placeholder` が一致することと、
エスケープ済み引用符を含む値が一致することを検査します。
`EmptyPeripheral.ThrowingProperty` は上流と同じ `PropertySettingException` を返します。
任意モデルのプロパティ設定および名前で参照する `OtherCpu` の実行時取得は残課題です。

範囲検証手順で `work=target/stm32f407-repl-property-binding` を使います。
`cargo-test.log` の `renode_property_binding_` 3 行がケース別結果です。

### REPL の組み込み sysbus 依存

固定上流の 2 件を一括で移植しました。依存先 `sysbus` は機器宣言の有無にかかわらず
最初から存在するため、コンストラクターの未解決参照や循環として扱いません。
sysbus 更新を伴うケースは上流と同様の模擬ハンドラーで初期化を確認します。
既存マシンの登録済みオブジェクトを変数として利用するには、ローダーがそのマシンを
受け取る経路が必要です。この対応は残課題です。

範囲検証手順で `work=target/stm32f407-repl-sysbus-dependency` を使い、
最後に `run sysbus tests/cortex-m4-sysbus.sh "$work/sysbus"` を加えます。
`cargo-test.log` の `renode_sysbus_dependency_` 2 行がケース別結果です。

### REPL の既存登録参照

固定上流の 2 件を一括で移植しました。あらかじめ模擬機器を登録したマシンで
元の REPL を検査し、既存名の再宣言を `VariableAlreadyDeclared` で拒否します。
既存 CPU の登録点 1 と新規 CPU の登録点 2 を保持し、
後者の `OtherCpu` が前者と同一参照であることを確認しました。
この検証経路の対象は上流の模擬機器です。実行中の一般マシンに対する REPL の
追加、失敗時に渡したマシンを再利用する API、任意モデル参照は残課題です。

範囲検証手順で `work=target/stm32f407-repl-existing-registration` を使います。
`cargo-test.log` の `renode_existing_registration_` 2 行がケース別結果です。

### REPL の引用符エスケープ

固定上流で `Ignore` になっている 3 件を、ソースにある入力と期待値で
ローカル移植しました。登録済み `MockCPU.Placeholder` の値を確認しています。
共有経路の統合と検証は ARM 担当に引き渡します。
`cargo test renode_quoted_fixture_` で再現できます。全体回帰とケース別結果は
`target/stm32f407-repl-legacy-quotes` にあります。

### STM32 CAN の生成とリセット

固定 Infrastructure の `STMCANTest.AnotherTest` を移植しました。上流の
`Ignore` 指定を台帳に保持したまま、元の操作と同じ CAN の生成とリセットを実行し、
例外がないことを確認します。固定上流のもう一つの無効化ケースは、
初期化モード外での CAN_BTR 操作と FINIT の扱いに不一致があり、未達です。

`cargo test renode_stmcan` で再現できます。全体テストと未変更 REPL を使う
FreeRTOS ELF のログと終了状態は `target/stm32f407-can-reset-fixture` に保存します。

### Cortex-M4F CMSIS-DSP の公開 ELF 13 件

固定 Renode の `tests/unit-tests/arm-dsp-examples-cortex-m4.robot` を読み、
元の Cortex-M4F・NVIC・ROM/RAM・TrivialUart 構成と ELF を使って一括実行します。
各 ELF のサイズと SHA-1 を配布名に照合します。全 13 件は
`UART.TrivialUart` の `TypeNotResolved` で停止し、台帳では未達です。
UART を LiteX に差し替えた切り分けでも `cortex-m4f` の指定が拒否されました。
Class Marks には UART 行の正規表現、残りにはリテラル照合が必要です。
上流との CPU 仮想時刻の対応と ELF 起動後の出力は未検証です。

```sh
work=target/stm32f407-cmsis-dsp
mkdir -p "$work"
cargo test > "$work/cargo-test.log" 2>&1
cargo build --release
python3 tests/cortex-m4-cmsis-dsp.py "$work" > "$work/exact.tsv"
tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu > "$work/freertos.log" 2>&1
```

現状の一括実行は終了コード 1 です。各行の `BLOCKED` に対応する
`arm_*_example.log` が個別の停止理由です。代表 1 件は実行コマンド末尾に
`--case arm_bayes_example` を加えて再現できます。ARM 担当への依頼は
`~/.local/state/penv-dispatch/shared-requests/stm32f407-cmsis-dsp-uart.md` です。

独立した STM32F4 I²C の HS3001 2 件も確認しました。
`using "platforms/cpus/stm32f4.repl"` に
`hs3001: Sensors.HS3001 @ i2c1 0x44` の宣言を加え、
`target/release/penv platform check --file "$work/hs3001/board.repl" --platform-root ../renode`
を実行すると、`TypeNotResolved: unsupported platform model "Sensors.HS3001"`
で停止します。センサーの実装、温湿度の設定と RESD ストリーム、元の Zephyr
ELF による UART 検証が残っています。

### STM32F4 I²C の HS3001

固定 Infrastructure `09ac7c563095c5410258ecc1b6b841a013db85` の
`HS3001.cs` を参照し、既定の 14 ビット測定と STM32F1 I²C の子アドレス登録を実装しました。
上流 `HS3001.robot` の Zephyr ELF を使用し、初回の
`temp is 0.00 °C humidity is 0.00 %RH` を、元の 8 秒の待機条件で
仮想時刻 0.058565 秒に確認しました。

```sh
work=target/stm32f407-hs3001
mkdir -p "$work"
cargo test > "$work/cargo-test.log" 2>&1
cargo build --release > "$work/build.log" 2>&1
tests/stm32f407-hs3001.sh "$work"
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

残りは 5 組の温湿度変更と UART 照合、別ケースの RESD チャネル 0 の
1 Hz 再生と末尾値保持、センサーの再設定モードです。元の刺激を適用する共有
モニターと RESD の対応を ARM 担当に依頼しました。初回出力のみを部分移植とし、
RESD ケースは未達です。

独立した AD4011 の 4 件では、同じ STM32F4 定義に
`adc: Analog.AD4011_ADC @ spi1` と `referenceVoltage: 4.82` を加えた最初の
プラットフォーム検査で型解決が停止します。再現ファイルとログは
`$work/ad4011.repl` と `$work/ad4011-platform.log` です。

### STM32F4 SPI の AD4011

固定 Infrastructure `09ac7c563095c5410258ecc1b6b841a013db85` の
`AD4011_ADC.cs` を基に、18 ビット変換、SPI の 3 バイト応答、設定レジスター、
直接接続の ADCChannelSource を実装しました。
上流の基準電圧 4.82 V と入力 0–4.5 V（0.5 V 刻み）を同じ ELF で確認し、
10 電圧すべてで `abs(measured - input) < 0.00005 V` を満たしました。
ELF は SPI の応答を USART2 に出力し、スクリプトが上流と同じ式で復号します。

```sh
work=target/stm32f407-ad4011
mkdir -p "$work"
cargo test > "$work/cargo.log" 2>&1
cargo build --release > "$work/build.log" 2>&1
tests/stm32f407-ad4011.sh "$work" > "$work/sweep.tsv"
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

直接入力ケースは各電圧の初期値を設定して個別に起動するため部分移植です。
元の Robot の動的な `Volts` と `Transmit`、浮動入力の選択、入出力 MUX の
操作・検証が残っています。共有モニターと追加 REPL 経路を ARM 担当に引き渡しました。

独立した ArrayMemory ケースは、元の `ArrayMemory.robot` の
`${PLATFORM}` を展開した `$work/array-memory.repl` を
`target/release/penv platform check --file "$work/array-memory.repl"`
で検査すると、CPU の `PerformanceInMips: 12` が拒否されます。
ARM 担当にはその再現と、後続の Save/Clear/Load・UART の条件を渡しました。

AD4011 の浮動入力ケースでは、STM32F4 SPI 上の ADC に AnalogWire を登録し、
浮動中の入力を除いて最小アドレスの入力を選択します。入力がすべて浮動なら
0 V です。固定 Robot の4状態をそれぞれ起動時に設定し、直接入力10点と合わせて
同じ SPI ELF で実行しました。両入力が有効なケースは登録の宣言順を逆にし、
アドレス0を優先することを確認しました。元のセッション内の `IsFloating` と
`Transmit` は共有モニター対応待ちのため部分移植です。

```sh
work=target/stm32f407-ad4011-wire
cargo build --release
tests/stm32f407-ad4011.sh "$work" --all
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

ログは `$work/all.tsv`、`$work/cargo.log`、`$work/freertos.log` です。
残る入出力 MUX は `$work/mux-Input.repl` と `$work/mux-Output.repl` を
`penv platform check --file PATH --platform-root ../renode` で検査すると、それぞれ
`Analog.MUX36S16_Input` と `Analog.MUX36S16_Output` の型解決で停止します。

AD4011 の入出力 MUX を STM32F4 SPI の接続構成に追加しました。
入力 MUX はチャンネル 0–15 の選択と無効時の 0 V を、出力 MUX は
出力5が選択され有効なときだけ ADC に入力を渡すことを確認しました。
元の初期宣言はどちらもプラットフォーム検査を通過します。

```sh
work=target/stm32f407-ad4011-mux
mkdir -p "$work"
cargo test > "$work/cargo.log" 2>&1
cargo build --release > "$work/build.log" 2>&1
tests/stm32f407-ad4011.sh "$work" --all > "$work/all.tsv"
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

1本の SPI ELF で直接入力10点、浮動入力4状態、入力 MUX 32状態、
出力 MUX 3状態を実行し、全49点が元の電圧誤差条件を満たしました。
MUX の選択・有効状態と出力接続は、検証用 REPL の起動時に設定しています。
元の Robot の同一マシン上の `OnGPIO`、`ConnectOutput`、`IsFloating` の戻り値、
動的登録・`Transmit` は共有モニター対応待ちのため、両ケースは部分移植です。

### Discovery 2台の F429 Zephyr TCP エコー

固定上流 `nucleo_f429zi.robot:28` の公開サーバー・クライアント ELF を
未変更 Discovery REPL でロードし、両機の Ethernet を仮想スイッチに接続します。

```sh
work=target/stm32f407-f429-echo-svd
mkdir -p "$work"
cargo test > "$work/cargo.log" 2>&1
cargo build --release
tests/stm32f407-f429-echo.sh "$work" > "$work/case.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

修正前は両機が `arch_system_halt` に入りました。サーバーの例外入口では
`CFSR=0x8200`、`BFAR=0x4001140c` で、USART6 CR1 の精密 BusFault です。
修正前の診断ログは `target/stm32f407-f429-echo/diagnose.log` にあります。
Discovery REPL の `ApplySVD` が参照する固定 STM32F40x SVD の USART6
レジスターを既存のタグ経路で扱うと、未変更 REPL のまま両機が起動し、
TCP 接続とペイロードの一致を確認できます。

500万ラウンドの回帰は `PARTIAL` として記録します。両機の UART は
共通 stdout にバイト単位で混在するため、各機の順序、8秒の待機、
4往復すべての確認には機器別 UART 照合が必要です。

別の `arm-cortex-m-fpu.robot:105` は
`target/release/penv platform check --file ../renode/platforms/cpus/nxp-k6xf.repl`
で `UART.K6xF_UART` の型解決が停止します。ログは `target/stm32f407-f429-echo/k6xf.log` です。
元の ELF で32仮想秒以内の `PROJECT EXECUTION SUCCESSFUL` を確認する条件が残っています。

### Cortex-M4 Thumb-2 データ処理

固定上流 `thumb2-data-processing-instr.robot:18` の公開 ELF をそのまま使い、
RAM と NVIC を保持して CPU を Cortex-M4 に設定します。1仮想秒後に
ELF シンボル `run_result` の16ビット値が `0xffff` か確認します。
現行コアでは PC `0x16c2` の `PKHBT r10, r8, r9, LSL #2` で停止し、
値は `0x0000` です。元の Cortex-M85 ケースとの等価性は未確認で、台帳では未達です。

```sh
work=target/stm32f407-thumb2-dp
mkdir -p "$work"
tests/cortex-m4-thumb2-dp.sh "$work" > "$work/focused.log" 2>&1
cargo test > "$work/cargo.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

別群の IT 状態のブロックフック2件も調べました。元の
`SetHookAtBlockBegin`、`SetHookAtBlockEnd` を含む RESC は、どちらも
コマンドの解決時に停止します。代表再現の `hook-Begin.resc` と
`hook-End.resc`、実測ログは同じ作業ディレクトリにあります。

### Discovery 上流 Zephyr MPU ELF

固定 `mpu.robot:73–82` の4本を台帳化し、未変更 Discovery REPL と
USART2 の `PROJECT EXECUTION SUCCESSFUL` を使う一括ランナーを追加しました。
通常実行は Robot の既定8仮想秒で照合します。今回は先に失敗箇所を切り分けるため、
同じバイナリで各100万ステップの実行をまとめました。

```sh
work=target/stm32f407-mpu-zephyr-bounded
mkdir -p "$work"
cargo build --release
PENV_MPU_DIAGNOSTIC_STEPS=1000000 tests/stm32f407-mpu-zephyr.sh "$work" > "$work/cases.tsv"
```

`cases.tsv` の数値はエミュレーター終了状態です。
`mem_protect` は禁止アクセスを通して予期しない fatal error、
`protection` はヒープ実行と書き込み禁止領域への書き込みを通して
`PROJECT EXECUTION FAILED` を出しました。`userspace` はモードと保護の
アサーションを失敗し、`stackprot` は最初の試験でスタックオーバーフローを報告しました。
保護判定とフォルト配送は ARM 担当に引き渡しました。
厳密な待機を使った最初の `mem_protect` 実行は、失敗出力を採取後に中断しました。
4件とも未達です。ログと生の UART は `$work` にあります。

別件の ArrayMemory は共有担当が起動前スナップショットまで実装しましたが、
元の Neonkey ELF の UART 完了行は同担当の検証でも未到達です。台帳に反映しました。

### Discovery の MPU 領域と背景マップ

固定 `mpu.robot:107–118` の3件を、未変更 Discovery REPL、元の MPU 領域、
PC、SP、ロード命令とステップ数で検証します。

```sh
work=target/stm32f407-mpu-regions
mkdir -p "$work"
PENV_F407_TIMED_RENODE_ROOT=../renode cargo test renode_f407_mpu_ -- --ignored --nocapture > "$work/cases.log" 2>&1
```

`PRIVDEFENA=1` は期待どおり `MMFSR=0` でした。`PRIVDEFENA=0` と
小領域の優先ケースは、期待する `MMFSR=0x82` が得られませんでした。
小領域ケースでは `MMFAR=0x20001020` も読めません。
背景マップの有効ケースを合格とし、保護違反の2件を未達としました。
同じ MPU 共有依頼に再現を追加しました。

別経路の `mpu.robot:85` は、配布 ELF を参照して元の
`cpu TranslateAddress 0xE000E000 Write` を含めた代表 RESC
`$work/busyloop.resc` で、モニターコマンド解決時に停止します。
元のユーザーモードとログ条件は未検証です。

### Cortex-M4 Semihosting と Thumb 命令カウンター

固定 `arm-semihosting.robot` の Cortex-M 対象25件を、元のメモリ配置と
共通／分割出力の定義で事前検査しました。担当機種に合わせて CPU は
Cortex-M4 を指定しています。両経路とも `CPU.SemihostingHandler` が
未解決です。`cases.tsv` はケースごとに対応する事前検査の結果を記録し、
実行結果とは区別します。

```sh
work=target/stm32f407-semihosting
mkdir -p "$work"
cargo build --release
python3 tests/cortex-m4-semihosting-preflight.py "$work" > "$work/cases.tsv"
```

別の切り分けでは、公開 WRITE0 ELF の SHA-1 を照合して実行しました。
`COMMON_OUTPUT.repl` から Semihosting の宣言2行を外した診断構成では、
PC `0x010004f0` の `BKPT #0xab` で停止し、`SUCCESS` は出ません。
ログは `$work/write0.log` です。Robot の入出力、終了状態、ファイル操作は未検証です。

別群の Thumb 命令カウンター2件は、16ビットと32ビットの元のパターンを
`cpu InstallOpcodeCounterPattern` に渡す代表 RESC で確認しました。
いずれもコマンド解決で停止します。再現ファイルとログは
`$work/opcode-{16,32}.resc` と `$work/opcode-{16,32}.log` です。
両群の共有経路を ARM 担当に引き渡しました。

### Cortex-M4 の LLVM Thumb アセンブル・逆アセンブル

固定 `llvm-asm-disasm.robot:317` の4展開を、元のアドレスと命令を使って
1個のオブジェクトにまとめ、外部 `llvm-mc` と `llvm-objdump` で照合しました。
`bic`、`ldr`、`mov.w`、`asrs` の符号化、命令長、オペランド、アドレスは
全件一致しました。元の monitor 経路では `AssembleBlock` と
`DisassembleBlock` が拒否されるため、台帳では部分移植です。

```sh
work=target/stm32f407-llvm-roundtrip
mkdir -p "$work"
python3 tests/cortex-m4-llvm-roundtrip.py "$work" > "$work/cases.tsv"
cargo test > "$work/cargo.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

別群の `page-access-via-io.robot:76` は、元の
`sysbus SetPageAccessViaIo 0x31000000` を含む代表 RESC でコマンド解決が
停止しました。元のページ切り替えとログ条件は未検証です。
診断 RESC は `$work/AssembleBlock.resc`、`$work/DisassembleBlock.resc`、
`$work/SetPageAccessViaIo.resc` です。`penv run --script PATH --max-steps 1`
で各コマンドの拒否を再現できます。共有経路は ARM 担当に引き渡しました。

### REPL の IRQ 取り消し・再配線

固定 `MergeAndCreationTests.cs:768–865` の5件から元の REPL を抽出し、
同じ penv バイナリで一括事前検査しました。4件では `MockIrqSender`、
範囲指定の1件では `MockGPIOByNumberConnectorPeripheral` が型解決で停止します。
取り消し後の接続数、受信先オブジェクト、IRQ 番号は未検証です。

```sh
work=target/stm32f407-irq-merge
mkdir -p "$work"
cargo build --release
python3 tests/stm32f407-irq-merge-preflight.py "$work" > "$work/cases.tsv"
cargo test > "$work/cargo.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream
```

別群の `InterruptHooks/arm-m.robot:8` は、元の F746G Discovery 構成を
`target/release/penv platform check --file ../renode/platforms/boards/stm32f7_discovery-bb.repl --platform-root ../renode`
で検査すると `Video.STM32LTDC` の型解決で停止します。
UART・GPIO の実ファームウェア検証やフックの始点と終点の照合は残っています。
実測は `$work/interrupt-hooks.log` です。両群とも未達です。
