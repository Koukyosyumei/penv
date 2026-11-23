# STM32F407 Discovery 上流ケース台帳

[ケース台帳](stm32f407-upstream-cases.tsv) は固定上流のリポジトリ、ファイル、ソース行、
ケース名、パラメーター、フィクスチャ、判定根拠または証拠を記録します。

| 出典 | 対象実行ケース | 上流等価合格 | 部分移植 | 未達 | 対象外 |
|---|---:|---:|---:|---:|---:|
| Renode | 513 | 78 | 0 | 435 | 237 |
| Infrastructure | 217 | 37 | 0 | 180 | 7 |
| **合計** | **730** | **115** | **0** | **615** | **244** |

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

未達 615 件には、共有フォールト配送、汎用 UART テスター、GDB・モニター・
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
`BusError::Unmapped` として検査します。周辺機器が生成した `CommandError` を
そのまま再送出する共有ケースと、その他のログモード・タグオプションは残件です。

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
