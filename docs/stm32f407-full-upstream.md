# STM32F407 Discovery 上流ケース台帳

[ケース台帳](stm32f407-upstream-cases.tsv) は固定上流のリポジトリ、ファイル、ソース行、
ケース名、パラメーター、フィクスチャ、判定根拠または証拠を記録します。

| 出典 | 対象実行ケース | 上流等価合格 | 部分移植 | 未達 | 対象外 |
|---|---:|---:|---:|---:|---:|
| Renode | 513 | 50 | 0 | 463 | 237 |
| Infrastructure | 217 | 1 | 0 | 216 | 7 |
| **合計** | **730** | **51** | **0** | **679** | **244** |

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

未達 679 件には、共有フォールト配送、UART ビット時間・パリティー、GDB・モニター・
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
CPU・デバッガー別の読出しを確認します。ロックアップからの NMI 復帰は残件です。

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
