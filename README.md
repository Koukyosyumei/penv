# penv

`penv` は、Renode のファームウェア実行機能を Rust で段階的に実装する CLI エミュレーターです。現在はベアメタル RV32IM を実行できます。外部クレートは不要です。

## ビルドと実行

```sh
cargo build --release
target/release/penv run --stats firmware.elf
target/release/penv run --raw --ram-mib 1 --max-steps 1000000 firmware.bin
target/release/penv run --help
```

ELF はリトルエンディアンの静的な RISC-V ELF32 実行ファイルを受け付けます。リンカースクリプトでロード対象の仮想アドレスを `0x80000000` から配置し、エントリーポイントを実行可能セグメント内に置いてください。raw イメージは同アドレスからロード・実行します。主 RAM の既定値は 16 MiB です。別アドレスにリンクした ELF では、たとえば `--map-ram 0x1000:0x40000` で RAM を追加できます。このオプションは複数回指定でき、容量の単位はバイトです。

ゲストが `0x10000000` にバイトを書き込むと、UART 出力として標準出力に流れます。診断と `--stats` の実行情報は標準エラー出力に出ます。`mtvec` を設定する前の `ECALL` と `EBREAK` は終了コード 0 で停止します。`mtvec` 設定後は、これらに加えて不正命令、未対応 CSR、命令フェッチとロード・ストアのアドレス例外をゲストのトラップハンドラーへ配送します。実行ステップ数が上限に達すると終了コード 2、実行・入力エラーでは 1 を返します。

`--clint 0x02000000` で`--harts` で指定した全ハート向けに CLINT を割り当てられます。ソフトウェア割り込みとタイマー割り込みに対応しています。タイマーはホストの経過時間に依存せず、実行ステップごとに進みます。割り込みへの遷移も 1 ステップとして上限に数えます。

`--harts N`（最大 8）で同じエントリーから複数のハートを開始できます。ハートごとにレジスターと CSR を持ち、共有 RAM に対して 1 ステップずつ順番に実行します。REPL からも 0 から連続する `hartId` の CPU を最大 8 個構成できます。CLINT は各ハートへ配線でき、PLIC と Murax の割り込みはハート 0 に接続できます。CLI から割り当てた CLINT はハートごとに独立したソフトウェア割り込みと比較レジスターを持ち、`mtime` は共有します。

## Renode のプラットフォーム定義

```sh
target/release/penv run --platform board.repl --platform-root ../renode firmware.elf
```

`--platform` は REPL のサブセットを読み込み、`CPU.RiscV32` または `CPU.VexRiscv`、RAM、任意の CLINT と Murax の UART、GPIO、タイマー を定義から構成します。`using` の相対パスは、定義ファイルの親ディレクトリ、指定されたルート（既定はカレントディレクトリ）の順に解決します。プロパティはインデント付きの行、または波括弧内にセミコロンで区切って記述できます。未対応のデバイスやプロパティはファイル名と行番号付きのエラーになります。CLINT の `frequency` は構文を検証し、進行速度には実行ステップを使います。プラットフォーム指定時のアドレスマップは REPL に従います。REPL の `UART.Murax_UART` は 32 ビットのデータレジスターから標準出力へ送信します。`--uart-input PATH` でファイルを受信 FIFO に入れられます。`IRQ -> cpu@11` を指定すると、ゲストが有効化した受信割り込みを配送します。

上流の `platforms/cpus/murax_vexriscv.repl` は変更せずに読み込めます。Murax のタイマーは実行ステップで進行し、GPIO はゲストから出力値を制御できます。外部 GPIO デバイスとの接続は未対応です。

Renode の `scripts/single-node/murax.resc` が参照するデモ ELF を使い、`--script` で同スクリプトからボード、トラップベクター、割り込みマスクを読み込めます。スクリプトが URL を参照する場合は、取得済みの ELF を最後の引数として渡すか、`--asset-dir` で参照先を指定します。`--uart-input-at` は、ファームウェアの初期化後に再現可能なタイミングで入力を届けます。

```sh
printf nt > uart.input
target/release/penv run --script ../renode/scripts/single-node/murax.resc \
  --platform-root ../renode \
  --uart-input uart.input --uart-input-at 1000 --max-steps 100000 murax.elf
```

上記では UART に `Ant` が出力され、命令実行を継続するデモをステップ上限で終了します。

上流の `platforms/cpus/riscv_virt.repl` も読み込めます。単一ハートで最大 2 コンテキスト（マシンとスーパーバイザー）の PLIC と、そこに接続された NS16550 UART を構成できます。検証用ファームウェアで UART 受信割り込みを受け取り、PLIC の claim と complete を行って受信バイトを送信し直すところまで確認しました。

上流の `platforms/cpus/litex_vexriscv_smp.repl` では、4 ハートから LiteX UART にハート ID を出力できることを確認しました。LiteX UART は即時送信とポーリング受信に対応し、バッファーの送信タイミングと IRQ 配線は未対応です。

上流の `scripts/single-node/litex_vexriscv_smp.resc` は、OpenSBI、Linux、DTB、initramfs を指定アドレスへ読み込み、4 ハートそれぞれの起動 PC を設定します。参照されるファイルを URL の末尾と同じ名前で保存し、`--asset-dir` で場所を指定します。

```sh
target/release/penv run --script ../renode/scripts/single-node/litex_vexriscv_smp.resc \
  --platform-root ../renode --asset-dir ./litex-assets --max-steps 400000000
```

Linux 5.0.9 の起動後、Buildroot へのログインとコマンド入力は別々のステップに指定できます。

```sh
printf 'root\r' > login.txt
printf 'cat /proc/cpuinfo\r' > cpuinfo.txt
target/release/penv run --script ../renode/scripts/single-node/litex_vexriscv_smp.resc \
  --platform-root ../renode --asset-dir ./litex-assets \
  --uart-event 600000000:login.txt --uart-event 675000000:cpuinfo.txt \
  --max-steps 725000000
```

同じステップに複数のイベントを指定した場合は、CLI に指定した順に受信 FIFO へ追加します。

## Cortex-M0 / Cortex-M0+

REPL の `CPU.CortexM` で `cpuType: "cortex-m0"` または `cpuType: "cortex-m0+"` を指定すると、限定的な Thumb 実行系を使用できます。`--raw` はイメージを `vectorTableOffset`（既定値 0）に読み込み、ベクターテーブルから初期 SP と PC を設定します。整数演算、分岐、即値オフセットでのロード・ストア、LiteX UART への出力を確認済みです。`BKPT` で実行を終了します。32 ビットの BL と、PUSH・POP、SP 相対のロード・ストアにも対応します。ARM EABI5 の静的 ELF32 は物理ロードアドレスを使用します。明示的なベクターアドレスがない場合は、ロードされる割り当て済みセクションの最小物理アドレスを使い、該当セクションがなければロードセグメントの先頭を使います。リセット時の SP と PC はそのベクターテーブルから取得します。起動後は VTOR への書き込みで、例外用のベクターテーブルをマップ済みの 128 バイト境界に移動できます。SVC はベクターテーブル経由でハンドラーを呼び出し、MSP を使って元の実行状態を復元できます。SysTick は実行ステップをクロックとして動作し、CPSID/CPSIE で割り込みをマスクできます。NVIC は 32 本の外部 IRQ の有効化、保留、優先度を管理し、REPL で LiteX UART を接続できます。高優先度の外部 IRQ または SysTick は実行中のハンドラーに割り込み、MSP で元のハンドラーに復帰できます。LDM/STM、DSB/DMB/ISB、および MSP、PRIMASK、PSR の MRS/MSR にも対応します。CONTROL.SPSEL でスレッドのスタックを切り替え、PSP を使った例外フレームから復帰できます。PendSV は ICSR から保留・解除でき、SHPR3 の優先度に従って配送されます。SVC のエスカレーション、CONTROL.nPRIV、その他の 32 ビット Thumb 命令は未対応です。

Cortex-M の REPL で `UART.STM32F7_USART` を複数配置し、各 USART を NVIC に直接接続するか、STM32F4 EXTI や CombinedInput を経由して接続できます。受信は USART と受信器を有効にした後に届けてください。標準入力用のファイルは、STM32 USART だけを配置した場合は最初に宣言した USART へ送られます。送信は即時で、DMA 配線は未対応です。

### STM32F072B Discovery

Renode の STM32F072B Discovery 用 Zephyr Hello World ELF と、上流の REPL をそのまま使って起動できます。以下の `hello.elf` は Renode の `tests/platforms/STM32F072b.robot` に記載された Hello World テスト用 ELF です。

```sh
target/release/penv run \
  --platform ../renode/platforms/boards/stm32f072b_discovery.repl \
  --platform-root ../renode --max-steps 1000000 --stats hello.elf
```

Renode の同じサンプルの Intel HEX イメージも `--hex` で読み込めます。Cortex-M の起動には REPL のベクターテーブルアドレスを使用し、指定がなければ HEX の最小データアドレスを使用します。

UART に `Hello World! stm32f072b_disco` が出力されます。サンプルは `WFI` で割り込みを待ち続けるため、ステップ上限で終了コード 2 を返します。

STM32F0 の REPL は RCC、GPIO レジスター、CRC、USART、EXTI、NVIC、SysTick、汎用タイマー、ADC、DMA、SPI、RTC、I²C、LED、ボタン、CAN を使用し、REPL の `Tag` で指定された未割り当て範囲では読み出しに 0 を返して書き込みを破棄します。タイマーの PWM・出力比較と入力キャプチャーを GPIO 配線に接続します。RCC のクロック設定はレジスターに反映されます。汎用タイマーは 1 ステップを 10 ns として REPL の周波数で進み、更新・比較割り込みを配送します。SysTick の周波数は REPL の `systickFrequency` に従います。指定を省略すると、1 ステップにつき 1 カウント進みます。Cortex-M は `WFE`、`SEV`、SCR の SLEEPONEXIT と SEVONPEND に対応します。SLEEPDEEP による電源モードの切り替えは未対応です。DMA はメモリー間転送と ADC・USART・タイマー・SPI の転送要求に対応します。SPI は FIFO、割り込みと `SPI.STP08ShiftRegister` の接続に対応します。シフトレジスター出力に登録した LED の変化も `--led-events` で表示します。RTC はカレンダー、サブ秒、アラーム、ウェイクアップタイマーおよびバックアップレジスターに対応します。I²C はマスター転送、再スタート、DMA、および `Memory.GenericI2cEeprom` の接続に対応します。ADC の入力電圧は `--adc-input 1:0.6` のように指定できます。

STM32F072 の 128 KiB フラッシュは、上流 REPL の `FLASH_INTERFACE` 範囲でアクセス制御、キーによるロック解除、ハーフワード書き込み、2 KiB ページ消去と全体消去に対応します。オプションバイトのキー解除、消去、書き込み、補数の読み出し、再ロードとセクター書き込み保護、および NVIC の完了・エラー割り込みに対応します。`OBL_LAUNCH` はオプションを再ロードしてリセットします。`AIRCR.SYSRESETREQ` はロード済みオプションを保持したままリセットします。どちらも SRAM、フラッシュの内容、RTC バックアップドメインを保持し、初期ベクターテーブルから起動します。オプションによるブート先の切り替えと読み出し保護は未対応です。

ボタンは `--button-event 100000:button:press --button-event 200000:button:release` のように操作でき、`--led-events` は LED の状態変化を標準エラー出力に表示します。 上流の Zephyr Button サンプルでボタン割り込みと LED の変化を確認しています。上流の ADC サンプルでは、`--adc-input 1:0.6 --adc-event 1000000:1:1.2 --max-steps 120000000` を指定すると、同じ実行で 745 と 1489 が順に出力されます。

### nRF52840 の上流 Zephyr Shell

nRF52840 用に Cortex-M4 のリセット、Thumb-2 の一部、BASEPRI・FAULTMASK、NVIC の第 2 IRQ バンク、共有 SRAM エイリアス、起動に必要な SCB レジスターと特権モードの MPU アクセス判定を実装しています。周辺機器は CLOCK、RTC0–2、UART/UARTE0–1、Watchdog、GPIO/GPIOTE、TWI0–1、SPI2 のレガシーマスターモードと ADXL372、TIMER0–4、I²S マスター、PDM、ローカル RADIO、RNG、ECB、EGU0–5 と、一部イベント・タスクを結ぶ PPI プログラマブルチャネルに対応します。

RTC と Watchdog は 32.768 kHz を 1 ステップあたり 10 ns の仮想時間で進めます。Watchdog は有効な全リロード要求の受理でカウンターを再ロードします。タイムアウト割り込みを有効にした場合は 2 LFCLK サイクル後にリセットします。UART の TX は即時に転送され、CLI 入力は最初に宣言した UART が受信を開始していれば SRAM に転送されます。

参照した Renode は `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`、モデルを確認した Renode Infrastructure は `09ac7c563095c5410258ecc1b1a6b841a013db85` です。

```sh
cargo test
cargo build --release
assets=$(mktemp -d)
resc=../renode/scripts/single-node/nrf52840.resc
url=$(sed -n 's/^\$bin?=@//p' "$resc")
elf="$assets/${url##*/}"
curl -fL "$url" -o "$elf"
printf '%s  %s\n' c00fbffd6b65c6238877c4fe52e8228c2a38bf1f "$elf" | sha1sum -c -
target/release/penv run --script "$resc" --platform-root ../renode \
  --asset-dir "$assets" --max-steps 1000000
```

未変更の上流 RESC と REPL を読み込み、`uart:~$` を表示します。`sysbus` の FICR/UICR タグは初期化コードの読み出しに使い、SVD の指定は診断用メタデータとして受理します。ステップ上限での終了コードは 2 です。`help` を送るには次のように実行します。

```sh
printf 'help\r' > "$assets/help.txt"
target/release/penv run --script "$resc" --platform-root ../renode \
  --asset-dir "$assets" --uart-event "100000:$assets/help.txt" --max-steps 1000000
```

同じ Shell のリセット回帰は、上記で取得した ELF と次のスクリプトで検証できます。

```sh
tests/nrf52840-shell.sh target/release/penv ../renode "$elf"
```

上流の UARTE テストと同じ `demo ping` を、リセット前後に UART から送信します。スクリプトは起動プロンプト、`pong`、応答後のプロンプトの順序を両方の起動で確認します。この ELF には再起動コマンドがないため、`--reset-at 300000` でリセットをかけます。`--reset-at STEP` は Cortex-M のリセットベクターから再起動し、CPU と周辺機器を既存のシステムリセット経路で初期化します。SRAM とフラッシュの内容、および今後のホスト入力イベントは保持し、同じステップの入力より先にリセットします。

Renode の `tests/platforms/NRF52840.robot` が参照する Watchdog ELF も同じ REPL で実行できます。

```sh
wdt_asset=nrf52840--zephyr-watchdog.elf-s_889852-75a696a301c8439400645e646a322039963d74c8
curl -fL "https://dl.antmicro.com/projects/renode/$wdt_asset" -o "$assets/watchdog.elf"
printf '%s  %s\n' 75a696a301c8439400645e646a322039963d74c8 "$assets/watchdog.elf" | sha1sum -c -
target/release/penv run --platform ../renode/platforms/cpus/nrf52840.repl \
  --max-steps 130000000 "$assets/watchdog.elf"
```

`Watchdog sample application` に続いて 5 回給餌し、`Waiting for reset...` の後に起動バナーと `Watchdog sample application` が再表示されます。デバッグ停止中の Watchdog と POWER のリセット理由レジスターは未実装です。GPIOTE はモデル内の GPIO レベルを使用し、外部ピン入力の接続は未実装です。CCM の固定 PPI チャネル、UARTE・TWI・SPI READY・TIMER COMPARE・RTC・I²S・PDM・RADIO・RNG・ECB・EGU 以外をイベント源とする PPI、UART のフロー制御・転送タイミング、MPU の非特権モードとフォルト例外配送も未実装です。サポートする MPU 設定でアクセス違反が起きると実行を診断付きで停止します。

TWI0/TWI1 は 7 ビットアドレスのレガシー TWI マスターモードです。REPL の NVIC IRQ 3/4 に接続し、アドレス NACK、受信・送信・停止・一時停止イベントと割り込み、繰り返し START、受信時の BB_STOP/BB_SUSPEND に対応します。`Memory.GenericI2cEeprom @ twi1 0x50` のように EEPROM を接続できます。TWIM EasyDMA、スレーブモード、ピン波形とバス速度の再現は未実装です。

```sh
# clang と ld.lld が必要
tests/nrf52840-twi.sh target/release/penv ../renode
```

このスクリプトは、上流の TWI 配線を含む一時 REPL でベアメタル ARM ELF を起動します。TWI0 で未接続アドレスの NACK と IRQ 3 を、TWI1 で IRQ 4、EEPROM の書き込み、繰り返し START からの 2 バイト読み出し、SUSPEND/RESUME/STOP を検証し、UART に `TWI OK` を出力します。上流 ADXL372 ELF は `Sensors.ADXL372 @ twi1 0x11` から識別子、ステータスと加速度を読み出します。

SPI2 は 8 ビット転送、2 バイトの受信バッファ、READY イベントと NVIC IRQ 35、PPI への READY イベント通知に対応します。上流モデルと同様に、ターゲット未接続時は TXD に書いても転送完了イベントは発生しません。`loopback: SPI.SPILoopback @ spi2` を追加すると、送信したバイトが受信バッファへ戻ります。`Sensors.ADXL372 @ spi2` は GPIO のアクティブ Low チップセレクトに対応します。SPIM3 は起動処理で使用する一部の設定レジスターに対応します。SPIM EasyDMA、ピン波形、センサー FIFO と物理入力は未実装です。

```sh
# clang と ld.lld が必要
tests/nrf52840-spi.sh target/release/penv ../renode
```

このスクリプトは、上流の SPI2 配線にループバックを追加した一時 REPL でベアメタル ARM ELF を起動します。2 バイトを連続送信し、NVIC 第 2 バンクの IRQ 35 を経て両バイトを順番に読み出し、UART に `SPI OK` を出力します。Shell のリセット回帰でも SPI2 を含めます。

TIMER0–4 は上流 REPL の比較チャネル数（4 または 6）を取り込みます。タイマーモードは 16 MHz を PRESCALER で分周し、1 ステップあたり 10 ns で進みます。COUNT によるカウンターモード、ビット幅、CAPTURE、COMPARE イベント、対応する NVIC IRQ、COMPARE_CLEAR/STOP ショートカットとプログラマブル PPI に対応します。低消費電力カウンターモードは通常のカウンターモードと同じ動作です。クロックの電源状態によるタイマーの停止は未実装です。

```sh
# clang と ld.lld が必要
tests/nrf52840-timer.sh target/release/penv ../renode
```

ベアメタル ARM ELF が TIMER0 の比較イベントで IRQ 8 を受け、同じイベントが PPI で TIMER3 の COUNT を駆動します。TIMER3 の第 6 比較チャネルで IRQ 26 を受け、両方のカウンターを CAPTURE して UART に `TIMER OK` を出力します。Shell のリセット回帰にも TIMER0–4 を含めます。

I²S はマスターの START/STOP、RXPTRUPD/TXPTRUPD/STOPPED イベント、NVIC IRQ 37 とプログラマブル PPI に対応します。EasyDMA は `RXTXD.MAXCNT` を 32 ビットワード数として扱い、開始時と次のバッファへの切り替え時にポインターをラッチします。転送間隔は MCK、LRCK 比、サンプル幅、チャネル数から計算します。入力未接続時は SRAM に無音を書き込み、TX は読み出したデータを破棄します。`loopback: true` を指定した検証構成では、TX から RX へパック済みワードをコピーします。

```sh
# clang と ld.lld が必要
tests/nrf52840-i2s.sh target/release/penv ../renode
```

ベアメタル ARM ELF は IRQ 37 で次のバッファを設定し、2 組のワード転送とバッファ境界、STOPPED を検証して `I2S OK` を出力します。スレーブモードとピン波形は未実装です。`--i2s-input` と `--i2s-output` は EasyDMA 形式の PCM バイト列を扱います。ファイル入出力を設定すると、上流モデルと同じく RX を SRAM に書き込んでから TX を採取し、バッファ転送時にポインターイベントを通知します。上流の hard-float I²S ELF は上流テストと同じ 32 バイトの入力をエコーし、出力がバイト単位で一致します。

PDM は EasyDMA の SAMPLE.PTR と SAMPLE.MAXCNT をバッファ境界でラッチし、16 ビットサンプルを書き込みます。STARTED/END/STOPPED は NVIC IRQ 29 とプログラマブル PPI に接続できます。`--pdm-input PATH` に符号付き 16 ビットの little-endian PCM を指定します。ステレオは左右交互に格納した PCM を使います。入力がない場合やファイル終端以降は無音になり、システムリセット時に入力位置を先頭へ戻します。STOP は転送途中のバッファを中止します。ピン波形、ゲインの信号処理、STOP 時の部分バッファ書き込みは未実装です。

```sh
# clang と ld.lld が必要
tests/nrf52840-pdm.sh target/release/penv ../renode
```

検証 ELF は IRQ 29 で次のバッファを設定し、3 サンプルずつの 2 組について PCM の値、ファイル終端後の無音、バッファ境界、STOPPED を検証して `PDM OK` を出力します。

上流の `Should Detect Yes Pattern` は、同じ ELF と PCM を使い UART の `Heard yes` を判定するスクリプトへ移植しました。

```sh
speech_asset=nrf52840--tflite-micro_speech.elf-s_7172308-9ab5781883d7af2582d7fea09b14352628da9839
pcm_asset=audio_yes_1s.s16le.pcm-s_32000-b69f5518615516f80ae0082fe9b5a5d29ffebce8
curl -fL "https://dl.antmicro.com/projects/renode/$speech_asset" -o "$assets/speech.elf"
curl -fL "https://dl.antmicro.com/projects/renode/$pcm_asset" -o "$assets/yes.pcm"
tests/nrf52840-pdm-upstream.sh target/release/penv ../renode "$assets/speech.elf" "$assets/yes.pcm"
```

未変更の上流 REPL で `Heard yes (204) @2560ms` を確認しました。Cortex-M4 は CPACR、単精度 VFP 演算、VFP レジスター転送、例外時の拡張フレームに対応します。FPU コンテキストは例外発生時に保存します。遅延保存、丸めモードの切り替え、浮動小数点例外の正確なフラグ、USB 通信、PWM 出力は未実装です。

上流の実 ELF と PCM を取得して SHA-1 を照合する回帰スクリプトを用意しています。UART/UARTE のリセット前後の `demo ping`、ハードウェア Watchdog による再起動とタスク Watchdog の 2 回連続再起動、WFE と Zephyr `kernel.interrupt`、I²S の PCM 一致、TWI/SPI の ADXL372、PDM の認識を確認できます。ADXL372 の 4 組の入力値は、REPL の軸プロパティを変えて独立した起動で検証します。

```sh
cargo test
cargo build --release
tests/nrf52840-audio-upstream.sh target/release/penv ../renode /tmp/nrf52840-audio-upstream
```

RADIO は TXEN/RXEN/START/STOP/DISABLE、READY/ADDRESS/PAYLOAD/END/DISABLED、TXREADY/RXREADY/PHYEND と IRQ 1 に対応します。PACKETPTR が指す SRAM 範囲を PCNF0/PCNF1 のヘッダーと長さ設定に従ってアクセスします。`loopback: true` の検証構成では最後の送信パケットを 1 件保管し、同じ周波数・MODE・パケット設定で受信を開始すると SRAM にコピーして CRCOK を立てます。READY_START や END_DISABLE などの一部ショートカットとプログラマブル PPI を使用できます。

```sh
# clang と ld.lld が必要
tests/nrf52840-radio.sh target/release/penv ../renode
```

検証 ELF は IRQ 1 で送信と受信の完了を確認し、送信側の元データを書き換えた後も受信データが一致すること、書き込みがバッファ境界を越えないこと、空の受信では END が発生しないことを確認して `RADIO OK` を出力します。ループバックは同期転送です。次の BLE ペア構成では仮想時刻で送受信イベントを駆動します。

上流 Bluetooth central ELF は未変更の上流 REPL で `Bluetooth initialized` と `Scanning successfully started` を表示します。Zephyr の `hal_radio_ram_prio_setup()` が使う AMLI RAMPRI（`0x40000e00–0x40000e34`）は読み書きを保持します。RAM アクセスは逐次実行されるため、バス競合による遅延はモデル化しません。`radio_reset()` の errata レジスターと、PHY、パケット、アドレス、CRC 設定の読み書き、RSSI とビットカウンターのイベントを追加しました。RTC の EVTEN で有効にしたイベントと、RADIO・TIMER・RTC・AAR 間の固定 PPI チャネル（20–23、26–31）も利用できます。Cortex-M4 側では CCR.UNALIGN_TRP に従う通常の RAM アクセスと、初期化に必要な Thumb-2 命令を追加しました。[Zephyr nRF52840 RADIO HAL](https://github.com/zephyrproject-rtos/zephyr/blob/30b4c29aaf5d/subsys/bluetooth/controller/ll_sw/nordic/hal/nrf5/radio/radio_nrf52840.h)

```sh
central=nrf52840--zephyr-bluetooth_central_hr.elf-s_3380332-316e27f81dcda3c2b0e7f2c3516001e7b27ad051
curl -fL "https://dl.antmicro.com/projects/renode/$central" -o "$assets/central.elf"
printf '%s  %s\n' 316e27f81dcda3c2b0e7f2c3516001e7b27ad051 "$assets/central.elf" | sha1sum -c -
tests/nrf52840-ble-central.sh target/release/penv ../renode "$assets/central.elf"
```

central 単体回帰は初期化とスキャン開始を判定します。接続と通知の回帰では、同じ未変更 REPL から 2 台の Cortex-M4 を作成し、`--ble-peer` でプロセス内の仮想無線に接続します。CPU は両ノードを 1 ステップずつ進め、UART の出力行に `primary:` と `peer:` を付けます。RADIO は周波数・PHY・アクセスアドレスが一致する受信機へパケットを渡し、ADDRESS と END を別の仮想時刻に発生させます。AAR はゲストの IRK を使い、解決イベントを IRQ 15 と PPI に配送できます。開始タスク内で解決し、実機の処理時間とスクラッチ RAM への書き込みは再現していません。

```sh
peripheral=nrf52840--zephyr-bluetooth_peripheral_hr.elf-s_3217940-7b59adc9629f8be90067b131e663a13d2d4bb711
curl -fL "https://dl.antmicro.com/projects/renode/$peripheral" -o "$assets/peripheral.elf"
printf '%s  %s\n' 7b59adc9629f8be90067b131e663a13d2d4bb711 "$assets/peripheral.elf" | sha1sum -c -
tests/nrf52840-ble-pair.sh target/release/penv ../renode "$assets/central.elf" "$assets/peripheral.elf"
```

上流 Robot ファイルの peripheral 名では `zephyr` の直後がアンダースコアになっています。取得できるハイフン区切りのファイルは、上流テスト記載の SHA-1 と一致します。ペア回帰では両ノードの接続、central の `[SUBSCRIBED]` と 2 バイトの心拍通知、peripheral の `HRS notifications enabled` を確認しました。RF 波形、物理 CRC、干渉・衝突、CCM による暗号化リンク、汎用の複数ノード RESC は未実装です。

RNG は START/STOP、VALUE、VALRDY イベント、NVIC IRQ 13、プログラマブル PPI、VALRDY_STOP ショートカットに対応します。VALUE と VALRDY は STOP 後も保持され、イベントはゲストがクリアします。固定シードの疑似乱数を、シミュレーション上の 64 µs 間隔（DERCEN を設定すると 128 µs）で生成します。DERCEN は生成間隔に反映し、物理エントロピーや統計的偏りの補正はモデル化していません。この出力は暗号用途に使用できません。

```sh
# clang と ld.lld が必要
tests/nrf52840-rng.sh target/release/penv ../renode
```

検証 ELF は最初の値が準備される前の STOP、連続して生成される 2 値と IRQ 13、INTENCLR でマスクした VALRDY、再有効化時の保留割り込み、VALRDY_STOP を確認して `RNG OK` を出力します。Zephyr Shell のリセット回帰にも RNG を含めます。

ECB は `ECBDATAPTR` で指定した SRAM 内の 48 バイトを、16 バイトの鍵、平文、暗号文の順に扱います。START で 1 ブロックの AES-128 暗号化を完了し、ENDECB をラッチします。領域外アクセスでは暗号文を書き込まず、ERRORECB をラッチします。両イベントは NVIC IRQ 14 とプログラマブル PPI に接続できます。転送は同期的に完了するため、STOP で中断できる処理やハードウェアの暗号化レイテンシーはモデル化していません。

```sh
# clang と ld.lld が必要
tests/nrf52840-ecb.sh target/release/penv ../renode
tests/nrf52840-rng.sh target/release/penv ../renode
tests/nrf52840-shell.sh target/release/penv ../renode "$elf"
```

検証 ELF は公開の AES-128 ECB ベクトル 2 ブロックをゲスト内で照合し、鍵・平文とバッファ外が保持されること、ENDECB、割り込みのマスクと再有効化、領域外バッファでの ERRORECB を確認して `ECB OK` を出力します。[NIST SP 800-38A F.1.1](https://nvlpubs.nist.gov/nistpubs/legacy/sp/nistspecialpublication800-38a.pdf)

EGU0–5 は各 16 本の TRIGGER タスクと TRIGGERED イベント、INTEN/INTENSET/INTENCLR、NVIC IRQ 20–25、プログラマブル PPI に対応します。検証 ELF は未変更 REPL を使い、EGU0 のイベントから PPI 経由で EGU5 を駆動し、IRQ 20/25 とイベント消去を確認します。

```sh
# clang と ld.lld が必要
tests/nrf52840-egu.sh target/release/penv ../renode
```

Renode の `Should Run Zephyr's kernel.interrupt Test` を移植しました。Cortex-M4 の NVIC STIR を使う IRQ 47/46 のネスト、動的 ISR、割り込みロックの検証を実行し、UART の `PROJECT EXECUTION SUCCESSFUL` を判定します。

```sh
irq_asset=nrf52840-zephyr-c320bb0-kernel-interrupt-test.elf-s_737512-7802727d11c415c1859c00842063ebec6c0a6fdc
curl -fL "https://dl.antmicro.com/projects/renode/$irq_asset" -o "$assets/kernel-interrupt.elf"
printf '%s  %s\n' 7802727d11c415c1859c00842063ebec6c0a6fdc "$assets/kernel-interrupt.elf" | sha1sum -c -
tests/nrf52840-kernel-interrupt.sh target/release/penv ../renode "$assets/kernel-interrupt.elf"
```

## 現在の実装範囲

RV32I 基本整数命令、M 拡張、Zifencei を実装しています。REPL で `rv32ima` または `rv32ima_zicsr_zifencei` を指定すると、共有 RAM 上の A 拡張命令を使用できます。`rv32imc` と `rv32imac` では RV32C の整数圧縮命令に対応します。`cycle`、`instret`、`time` と各上位 32 ビットを読み取れます。CLINT がある場合の `time` は `mtime` に従います。Zicsr の命令と、マシンモードの CSR の一部、および `ECALL`・`EBREAK` からの `MRET` を実装しています。`CPU.RiscV32` ではユーザーモードへの移行とマシンモードへのトラップ、16 エントリーの PMP に対応しています。スーパーバイザーモードでは同期例外の委譲と `SRET`、32 ビット物理アドレス範囲の Sv32 ページ変換に対応しています。変換はハートごとに 64 エントリーをキャッシュし、SFENCE.VMA でそのハートのキャッシュを無効化します。マシンモードから設定したスーパーバイザー割り込みを委譲できます。3 コンテキスト以上の PLIC、その他の ISA 拡張、REPL のその他の構文とデバイス、起動コマンド以外の RESC 構文、ネットワークと汎用の複数ノード RESC は未対応です。標準構成の UART は単一の送信レジスターを提供します。

RV32I の仕様は [RISC-V ISA マニュアル](https://docs.riscv.org/reference/isa/unpriv/rv32.html) を参照してください。
