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

#### 上流ファームウェアでの RESC 検証

Renode `c38ac3ba` の `tests/unit-tests/hex.robot` から起動コマンドを RESC に転記しました。変更前のログでは 3 行目の `sysbus LoadHEX` が `unsupported monitor command` になり、切り分けのために `LoadELF` に置き換えて実行すると 4 行目の `sysbus.cpu VectorTableOffset` も同じエラーになりました。

`LoadHEX` は既存の Intel HEX ローダーを使用します。`VectorTableOffset` には Cortex-M CPU 名と 128 バイト境界のアドレスを指定します。指定したアドレスは REPL とイメージから決まる既定値より優先され、初期 SP と PC の取得と例外ベクターの参照に使用されます。

```sh
cargo test
cargo build --release
assets=$(mktemp -d)
hex_url=$(awk '/Create Machine +https:/ {print $NF}' ../renode/tests/unit-tests/hex.robot)
curl -fL "$hex_url" -o "$assets/${hex_url##*/}"
cat >"$assets/hello.resc" <<EOF
mach create
machine LoadPlatformDescription @platforms/boards/stm32f072b_discovery.repl
sysbus LoadHEX @$hex_url
sysbus.cpu VectorTableOffset 0x08000000
EOF
target/release/penv run --script "$assets/hello.resc" --platform-root ../renode \
  --asset-dir "$assets" --max-steps 10000000 --stats
```

`--asset-dir "$assets"` の代わりに、末尾の引数に `"$assets/${hex_url##*/}"` を渡しても同じ HEX を起動できます。両方の RESC 実行で `--hex --platform` による直接起動と同一の UART 出力を確認しました。回帰テストでは REPL と異なるベクターテーブルを RESC で指定して、ローカルイメージの両方の指定方法で起動を確認します。単体テスト 39 件と CLI テスト 6 件が成功しました。この検証環境では `cc` がなかったため、Cargo の実行時に `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc` を設定しました。

同じ上流ツリーの `tests/platforms/STM32F072b.robot` が参照する ELF も確認しました。

```sh
awk '/Create Machine +.*\.elf/ {print $NF}
     /sysbus LoadELF @https:.*stm32f0-crc-test/ {sub(/^.*\//, ""); print $1}' \
    ../renode/tests/platforms/STM32F072b.robot |
while IFS= read -r name; do
  curl -fL "https://dl.antmicro.com/projects/renode/$name" -o "$assets/$name"
done

demo() {
  name=$1; steps=$2; shift 2
  target/release/penv run \
    --platform ../renode/platforms/boards/stm32f072b_discovery.repl \
    --platform-root ../renode --max-steps "$steps" --stats "$@" "$assets"/*"$name".elf-*
}
printf 'Echo working?\r' >"$assets/echo.input"
demo hello_world 1000000
demo echo 3000000 --uart-input "$assets/echo.input" --uart-input-at 1000000
demo blinky 550000000 --led-events
demo button 1000000 --button-event 100000:button:press --button-event 200000:button:release --led-events
demo adc 120000000 --adc-input 1:0.6 --adc-event 1000000:1:1.2
demo crc-test 150000000
```

Hello World の UART 出力、Console Echo の `Echo working?`、Blinky の LED 点灯と消灯の繰り返し、Button の `Button pressed at` と LED 遷移、ADC の `745` と `1489`、CRC の `test result: ok. 840 passed; 0 failed` を確認しました。各実行はステップ上限で終了コード 2 を返しました。

RESC で指定する HEX または ELF は 1 イメージで、URL の実ファイルは事前に取得します。

#### RESC での Button と ADC の回帰確認

上流の Button と ADC シナリオを RESC に転記すると、変更前の実行ログでは `emulation RunFor`、`sysbus.gpioPortA.button Press`、`sysbus.adc.adc-channel1 Volts 0.6` がそれぞれ `unsupported monitor command` になりました。

起動設定の後に `emulation RunFor "0.3s"`、ボタンの `Press` / `Release`、ADC の `Volts` を順番に実行できます。Cortex-M の `RunFor` は小数秒を 10 ns 単位の実行ステップに換算し、区間をまたいで CPU と周辺機器の状態を保持します。全区間で `--max-steps` を共有し、途中で上限に達すると終了コード 2、全操作を処理すると 0 を返します。

上の手順で取得したファームウェアを使います。

```sh
resc_boot() {
  printf 'mach create\nmachine LoadPlatformDescription @platforms/boards/stm32f072b_discovery.repl\nsysbus LoadELF @%s\n' "$1"
}
resc_boot "$assets"/*button.elf-* >"$assets/button.resc"
cat >>"$assets/button.resc" <<'EOF'
emulation RunFor "0.001s"
sysbus.gpioPortA.button Press
emulation RunFor "0.3s"
sysbus.gpioPortA.button Release
emulation RunFor "0.001s"
sysbus.gpioPortA.button Press
emulation RunFor "1s"
sysbus.gpioPortA.button Release
emulation RunFor "1s"
sysbus.gpioPortA.button Press
emulation RunFor "1s"
sysbus.gpioPortA.button Release
emulation RunFor "0.001s"
EOF

resc_boot "$assets"/*adc.elf-* >"$assets/adc.resc"
cat >>"$assets/adc.resc" <<'EOF'
emulation RunFor "0.01s"
sysbus.adc.adc-channel1 Volts 0.6
emulation RunFor "1.05s"
sysbus.adc.adc-channel1 Volts 1.2
emulation RunFor "1.05s"
EOF

target/release/penv run --script "$assets/button.resc" --platform-root ../renode \
  --max-steps 331000000 --led-events >"$assets/button.out" 2>"$assets/button.err" &&
  grep -q '^Button pressed at ' "$assets/button.out" &&
  awk '/^LED led / { states = states " " $3 }
       END { exit (states != " on off on off on off on") }' "$assets/button.err"

target/release/penv run --script "$assets/adc.resc" --platform-root ../renode \
  --max-steps 211000000 >"$assets/adc.out" &&
  tr -d '\r' <"$assets/adc.out" |
  awk '/Booting Zephyr/ { boot = 1 }
       boot && /^ADC reading: 745$/ { first = 1 }
       first && /^ADC reading: 1489$/ { second = 1 }
       END { exit !second }'
```

両 RESC が終了コード 0 を返しました。Button は UART に押下時刻を出力し、1 秒の保持区間を挟んで期待どおりに LED が遷移しました。ADC は起動直後の `0` に続いて `745`、`1489` を出力しました。`cargo test` では複数区間の継続実行、ステップ上限とデバイス名エラーの行番号を含む全 45 件が成功しました。

Console Echo でも `resc_boot "$assets"/*echo.elf-*` に `emulation RunFor "0.01s"` と `emulation RunFor "0.02s"` を追加し、上で作成した入力ファイルを `--uart-input "$assets/echo.input" --uart-input-at 1000000` で渡しました。UART の入力案内に続いて `Echo working?` が出力され、終了コードは 0 でした。

`RunFor` の期間指定は 10 ns 単位で表せる小数秒に対応します。

#### UART 出力を待って ADC を操作する回帰

上流の `Should Read ADC` は、起動メッセージと最初の測定値を受け取ってから電圧を変更します。変更前は、これを転記した `tests/fixtures/stm32f072-adc.resc` の 4 行目で UART の待機操作が未対応でした。penv の RESC 拡張 `waitForUart "期待文字列"` で、未消費の LF または CRLF 終端の行をリテラル部分一致で検査します。履歴に一致があればその場で進み、新規出力で一致した場合はその行の終端でゲストの実行を区切ります。待機も `--max-steps` を共有します。上限到達やゲスト停止までに一致しなければ、RESC の行番号と期待文字列を出力して終了コード 1 を返します。

```sh
assets=$(mktemp -d)
adc_url=$(sed -n 's/^sysbus LoadELF @//p' tests/fixtures/stm32f072-adc.resc)
curl -fL "$adc_url" -o "$assets/${adc_url##*/}"
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS="$assets" \
  cargo test --release upstream_stm32f072_adc_waits_for_uart_between_voltage_changes -- --ignored
```

公開 ELF で `Booting Zephyr`、`ADC reading: 745`、`ADC reading: 1489` の順序と、最後の一致行での停止を確認しました。ステップ上限を 1 にした実行では、最初の待機で失敗することも確認しています。通常の `cargo test` は外部ファイルなしで実行でき、公開 ELF の回帰は上のコマンドで実行します。

UART の選択方法と待機の残課題は、下の Console Echo の節に記載しています。

#### USART への RESC 入力で Console Echo を再現

上流の `Should Run Zephyr Console Echo Sample` を、上流 REPL と公開 ELF を使う `tests/fixtures/stm32f072-echo.resc` に転記しました。変更前のログで最初に失敗した操作は 5 行目の `sysbus.usart1 WriteLine "Echo working?"` でした。

`WriteLine` は REPL 名で STM32 USART を選択し、Renode の既定と同じ CR を付けて受信 FIFO に入力します。この回帰では `waitForUart` の対象を `sysbus.usart1` に指定して入力案内を待ち、`sysbus.usart1 WriteLine "Echo working?" CRLF` を実行します。その後、受信割り込みを処理したファームウェアが返す `Echo working?` を同じ USART で待ちます。CLI で入力ステップを指定せずにシナリオを再現できます。

```sh
assets=$(mktemp -d)
echo_url=$(sed -n 's/^sysbus LoadELF @//p' tests/fixtures/stm32f072-echo.resc)
curl -fL "$echo_url" -o "$assets/${echo_url##*/}"
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS="$assets" \
  cargo test --release upstream_stm32f072_console_echo_accepts_resc_uart_input -- --ignored
```

公開 ELF で案内の次の行が `Echo working?` になり、終了コード 0 を返すことを確認しました。公開 ADC ELF の既存回帰と Hello World HEX の RESC 起動も再確認しました。通常の `cargo test` は 48 件成功しました。これには名前で選んだ USART だけに本文と CR が届くテストと、フラッシュのロック、半ワード書き込み、オプション再ロードでの保持、ページ消去の確認が含まれます。

`waitForUart sysbus.usart1 "期待文字列"` は、指定した USART が送信した LF または CRLF 終端の行だけを照合します。標準出力にはすべての UART 出力が流れます。USART を省略した従来の形式は、その標準出力を照合します。

`WriteLine "文字列"` の後ろに `CR`、`LF`、`CRLF`、`None` を指定できます。省略と `true` は CR、`false` は行終端なしです。Console Echo の公開 ELF で明示的な CRLF を、ADC の公開 ELF で USART を指定した `Booting Zephyr` → `745` → `1489` の待機を確認しました。両 ELF を用意済みの場合は、上のテスト名を `upstream_stm32f072` にすると両方を実行できます。通常の `cargo test` は 50 件成功し、別ポートからの同一文字列と行終端引数の全指定を検査します。

#### CR で終端する UART 出力の待機

`waitForUart sysbus.usart1 "期待文字列" CR` は、対象 USART が CR を送信した時点で行を照合します。USART 名を省略して標準出力全体を照合する場合も `CR` を指定できます。終端引数の省略または `LF` は従来どおり LF を待ち、LF と CRLF の行に対応します。CRLF の出力に `CR` を指定した場合は、後続の LF より先に進みます。

```sh
cargo test
assets=$(mktemp -d)
for sample in echo adc; do
  url=$(sed -n 's/^sysbus LoadELF @//p' "tests/fixtures/stm32f072-$sample.resc")
  curl -fL "$url" -o "$assets/${url##*/}"
done
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS="$assets" \
  cargo test --release upstream_stm32f072 -- --ignored

cargo build --release
sed '$s/$/ CR/' tests/fixtures/stm32f072-echo.resc >"$assets/echo-cr.resc"
target/release/penv run --script "$assets/echo-cr.resc" --platform-root ../renode \
  --asset-dir "$assets" --max-steps 3000000 >"$assets/echo-cr.out"
tail -c 14 "$assets/echo-cr.out" | od -An -tx1
```

通常の `cargo test` は 51 件成功しました。CR の直後に BKPT を実行する STM32 USART のテスト用 HEX で、USART 指定あり・なしの両方が CR で終了し、既定の LF 待機はゲスト停止で失敗することを確認しました。公開 Console Echo と ADC の既存回帰も成功し、最後の待機だけを CR にした Console Echo は `Echo working?` の直後の `0d` で終了しました。

#### UART 履歴と消費位置

Renode の `Wait For Line On Uart` は、既定で未消費の完了行を先頭から探します。成功すると最初の一致行までを消費し、後続行と書きかけの行を次の待機に残します。参照した実装は `CheckFinishedLines` と `HandleSuccess` です。[Renode TerminalTester](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Main/Testing/TerminalTester.cs)

penv は最初の実行区間から、スクリプトで待機対象にした USART と、UART 名を省略した場合の統合出力をそれぞれ独立に記録します。履歴の一致はステップを消費しません。一致した終端までを消費し、後続のバイトを残します。未消費の履歴は対象ごとに 64 KiB で、上限を超えた分は最も古いバイトから破棄します。

Console Echo の回帰は、`RunFor` 中に出た起動行と入力案内を順に履歴から消費した後、入力への応答を待ちます。上の手順で公開 ELF を取得し、同じ `cargo test --release upstream_stm32f072 -- --ignored` で Console Echo と ADC を検証できます。ADC でも、起動行を待つ前に `RunFor` を挿入した次の変形を確認しました。変更前は、起動行が標準出力に出ていても 5 行目の待機が失敗しました。

```sh
sed '4i emulation RunFor "0.01s"' tests/fixtures/stm32f072-adc.resc >"$assets/adc-backlog.resc"
target/release/penv run --script "$assets/adc-backlog.resc" --platform-root ../renode \
  --asset-dir "$assets" --max-steps 220000000
```

公開 ELF の回帰 2 件が成功し、ADC の変形でも `745` と `1489` が順に出力されました。通常の `cargo test` は 53 件成功し、消費済み行の再利用を防ぐこと、残りステップ 0 での履歴照合、書きかけの行の継続、履歴の上限を確認しました。

#### 改行を待たずに UART プロンプトを照合する

`waitForUartPrompt sysbus.usart1 "期待文字列"` は、Renode の `Wait For Prompt On Uart` と同様に、履歴の完了行を先に検索し、続いて未完了行を照合します。まだ一致がなければ、入力された文字列が一致した時点で改行を待たずにゲスト実行を区切ります。履歴の未完了行に一致した場合は、その時点までに受信していた行全体を消費します。USART 名を省略すると統合出力を使います。通常の `waitForUart` は行終端を待ちます。

上の手順で Console Echo と ADC の公開 ELF を取得し、Console Echo のライブプロンプト、履歴を使う Console Echo、ADC の行待機を実行します。プロンプトの直後でスクリプトを終えると、末尾が `back`（`62 61 63 6b`）になり、ゲストがまだ CR/LF を送っていないことを確認できます。

```sh
cargo test
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS="$assets" \
  cargo test --release upstream_stm32f072 -- --ignored
cargo build --release
sed -n '1,5p' tests/fixtures/stm32f072-prompt.resc >"$assets/prompt-stop.resc"
target/release/penv run --script "$assets/prompt-stop.resc" --platform-root ../renode \
  --asset-dir "$assets" --max-steps 3000000 >"$assets/prompt-stop.out"
tail -c 4 "$assets/prompt-stop.out" | od -An -tx1
```

`cargo test` は 55 件成功し、公開 ELF の回帰 3 件も成功しました。Console Echo では改行より前に入力案内を照合してから入力し、`Echo working?` を完了行として受信しました。ADC では引き続き `745` → `1489` を完了行として確認しました。テスト用 HEX でも、改行のない出力 `A` に対し、ライブと履歴の両方から待機が完了することを確認しました。

プロンプト照合の正規表現、STM32 USART 以外の名前付き待機、Robot の UART 入力・LED テスターは残課題です。

### 固定した上流 ARM 回帰の対応表

参照元は Renode の `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` に固定しました。以下のパスはこのリビジョンからの相対パスです。[Renode の固定リビジョン](https://github.com/renode/renode/tree/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests)

棚卸しの対象は、STM32F072B Discovery を直接使う Robot ケース全件、Cortex-M0/M0+ で実行できる命令・例外・割り込みの観測条件、および F072 と観測条件を共有する STM32 USART・フラッシュのケースです。別 SoC のフルシステム、TrustZone、FPU、CPU ごとのメモリ登録に依存するケースは移植の対象外としました。

#### STM32F072B Discovery

最初の 6 件は `tests/platforms/STM32F072b.robot`、最後の 1 件は `tests/unit-tests/hex.robot` です。いずれも `tests/fixtures/stm32f072-*.resc` から、上流の変更していないボード REPL と公開イメージを使用します。

| 上流ケース | 移植状況 | 観測条件 |
| --- | --- | --- |
| `Should Run Zephyr Hello Sample` | 今回追加。 `upstream_stm32f072_hello_world_boots_elf_and_hex` の ELF 実行 | `Hello World! stm32f072b_disco` の完了行。 |
| `Should Run Zephyr Console Echo Sample` | 既存。 `upstream_stm32f072_console_echo_accepts_resc_uart_input` と `upstream_stm32f072_console_echo_replies_after_uart_prompt` | 入力案内を受けて送信し、次に `Echo working?` を受信。 |
| `Should Run Zephyr Blinky Sample` | 今回追加。 `upstream_stm32f072_blinky_alternates_led_states` | 4 秒の実行区間における LED の点灯・消灯 3 往復。Robot の状態ごとのタイムアウト判定は残課題。 |
| `Should Run Zephyr Button Sample` | 今回追加。 `upstream_stm32f072_button_updates_led_and_reports_press` | 3 回の押下・解放、数値の押下時刻を含む UART 行、LED の遷移列。1 秒の保持区間を含み、検査は遷移ログを使う。Robot の各操作での保持判定は残課題。 |
| `Should Read ADC` | 既存。 `upstream_stm32f072_adc_waits_for_uart_between_voltage_changes` | 起動行、0.6 V で `745`、1.2 V で `1489`。 |
| `Should Run stm32f0-crc-test Application` | 今回追加。 `upstream_stm32f072_crc_firmware_passes_840_cases` | `test result: ok. 840 passed; 0 failed` の完了行。 |
| `Should Load HEX` | 今回追加。 `upstream_stm32f072_hello_world_boots_elf_and_hex` の HEX 実行 | `LoadHEX`、明示的な `VectorTableOffset`、同じ Hello World 完了行。 |

`tests/unit-tests/per-core-registration.robot` の `Should Load Hex To Core Specific Memory` と `Should Not Load Hex To Invalid Core Specific Memory` は同じ HEX を参照します。CPU ごとのメモリ登録とロード先指定が必要なため、上表の単一 CPU の HEX 回帰とは別の残課題です。

#### Cortex-M と STM32 周辺機器

| 分野 | 上流のケース | penv の対応と検証範囲 |
| --- | --- | --- |
| Thumb-2 | `tests/unit-tests/tlib/arm/thumb2-data-processing-instr.robot` の `Should Run All Instructions` | `renode_thumb2_fixture_branches_and_reads_apsr` に ELF の `BL`（`0x1002`、`0x1006`）と `MRS APSR`（`0x1a1e`）の命令語を移植しました。分岐先と LR を検査し、ORRS ケースが期待する N/Z/C を事前設定して APSR の読出しを検査します。 |
| Thumb 状態 | `tests/unit-tests/arm-thumb.robot` の `Should Expose Thumb State In CPSR`、`tests/unit-tests/arm-thumb2-srs-rfe.robot` の SRS/RFE | Cortex-A の ARM/Thumb 切り替え、CPSR、SRS/RFE は対象外です。既存の `runs_cortex_m0_from_vector_table_and_reports_invalid_thumb_state` はリセットベクターの Thumb ビットを検査します。 |
| 例外復帰 | `tests/unit-tests/arm-cortex-m-exception-return.robot` の `Should Return From Exception When Using Allowed Instruction` | 非 TrustZone の `BX LR` を `renode_exception_bx_lr_restores_interrupted_thread` に移植しました。上流の NMI を Cortex-M0 の外部 IRQ に置き換え、ハンドラーへの遷移とスレッドの PC・SP の復帰を検査します。TrustZone、MPU、HardFault のケースは見送りました。 |
| 例外とイベント | `tests/unit-tests/arm-cortex-m-interrupts.robot` の `Should Wake Up From WFE When SEVONPEND Is Set` | 移植済み。 `renode_wfe_wakes_on_new_pending_irq_only_with_sevonpend` は上流と同じ IRQ 5、書込みアドレス、データを使い、SEVONPEND の有無を比較します。 |
| SysTick | 同ファイルの `Should Count Down SysTick With Reload Set Before Reset` と `Should Count Down SysTick With Reload Set After Reset` | 今回追加。 `renode_systick_counts_and_reloads_for_both_cvr_write_orders` で CVR/RVR の両順序、カウントダウン、COUNTFLAG の読出しによる解除、再ロードを検査します。4 実行ステップを 1 ティックに縮尺し、penv ではゼロを観測してから次ティックで再ロードします。上流の仮想時間境界と同じ結果を保証するテストは残課題です。 |
| NVIC のリセット | 同ファイルの `NVIC Base Priority Mask Setting Should Not Survive Reset` | 対象外。BASEPRI は現行の Cortex-M0/M0+ の範囲外です。 |
| Fault、クロック信号 | `tests/unit-tests/arm-cortex-m-bus-fault.robot` と `tests/unit-tests/arm-cortex-m-signals.robot` | Fault の例外配送・ロックアップ、Cortex-M Complex の外部クロックゲートと DWT は未移植です。既存の CLI テストは無効なリセットベクターをエラーとして検査します。SysTick の停止・再開を下表で検査します。 |
| UART 受信 | `tests/peripherals/STM32_UART.robot` の `Should Drop Characters When UART Or Receiver Disabled` | `renode_stm32_usart_drops_input_if_either_enable_is_clear` は F072 のレジスター配置で、上流の `0x12` と `0x34` が受信器または USART の無効中に破棄され、再有効化しても現れないことを検査します。フレーム時間、パリティ、アイドル検出の上流ケースは対象外です。 |
| UART 端末 | `tests/platforms/STM32F072b.robot` の Console Echo と ADC、`tests/unit-tests/uart-tester.robot` の端末選択 | 既存の公開 ELF 回帰は Echo、プロンプト待機、ADC をカバーします。`uart_watch_ignores_other_ports_without_filtering_stdout` は端末選択に対応する USART 分離を検査します。Robot のテスター ID とバイナリ・正規表現モードは未移植です。 |
| STM32 フラッシュ | `tests/unit-tests/stm32f4-erase.robot` の `Should Ignore Erase When MER And SER Are Reset`、`Should Perform Mass Erase`、`Should Perform Sector Erase`。隣接ページの観測条件は `tests/platforms/STM32L072.robot` の `Should Handle Flash Operations` | `renode_stm32_flash_erase_modes_preserve_the_expected_ranges` は F072 の 128 KiB バンク、2 KiB ページ、消去値 `0xff` を使用します。消去モードなし、指定ページ全体と隣接ページ、バンク全消去を検査します。F4/L0/H7 固有のコントローラーとフラッシュ用 ELF は対象外です。既存の `stm32f072_flash_preserves_programmed_data_on_reload_and_erases_a_page` は F072 のプログラムと再ロードを検査します。 |

STM32 フラッシュの消去操作は Rust のバス回帰で検証しています。

#### NVIC・SysTick・例外の追加移植

同じ固定リビジョンで、前回の `BX LR`、`SEVONPEND`、SysTick の CVR/RVR 書き込み順の回帰に加え、次の観測条件を移植しました。

| 上流のケース（`tests/unit-tests/` 内） | 追加した Rust テストと差分 |
| --- | --- |
| `arm-cortex-m-exception-return.robot` の `Should Not Return From Exception When In Thread Mode` と `Should Not Return From Exception When Not Using Allowed Instruction` | `renode_thread_bx_and_handler_blx_do_not_unstack` は、スレッドの `BX` とハンドラーの `BLX LR` が Cortex-M0 の `0xfffffff9` を分岐先として扱い、例外フレームを解除しないことを検査します。TrustZone の `BLXNS` は対象外です。 |
| 同ファイルの `Should Not Return From Exception When Loading PC From Vector Table` | `renode_exception_vector_load_does_not_unstack_an_exc_return_value` は、上流の MPU/HardFault の準備を外部 IRQ に置き換えます。ベクター内の `0xfffffff9` が PC にロードされ、IRQ がアクティブでスタックが保持されることを検査します。 |
| `profiler-trace.robot` の `Should Disable Profiler After Reaching Maximum Nested Context Count` | `renode_profiler_fixture_pendsv_dispatches_from_icsr` は、先頭の PendSV を抜き出し、上流と同じ ICSR とベクター・ハンドラーのアドレスで、要求の設定・取り消し・受理・復帰を検査します。MPU の MemManageFault とプロファイラーの判定は対象外です。 |
| `cpu-pause-halt.robot` の `CPU Should Halt and Reset on Cortex-M` と `CPU Should Halt Pause and Reset on Cortex-M` | `renode_cortex_m_reset_observation_via_sysresetreq` は、CPUWAIT やモニターの停止操作を WFI と AIRCR に置き換えます。VECTKEY の検証、リセットベクターへの復帰、NVIC と SysTick レジスターの初期化、RAM の保持を検査します。 |
| `arm-cortex-m-signals.robot` の `Should Stop NVIC SysTick and DWT CycleCounter When Not Clocked` | `renode_systick_counter_stops_and_resumes_via_enable` は、SysTick の観測条件を CSR.ENABLE で検査します。カウンターが停止中に変化せず、再開時に残っていたティック位相から進むことを確認します。外部クロックゲートと DWT は対象外です。 |

未移植のままの主なケースは、`arm-cortex-m-signals.robot` の `Should Persist Clocked State For Cortex-M Complex On Reset` と CPUWAIT、`arm-cortex-m-interrupts.robot` の BASEPRI、`it-status.robot` の IT ブロック中の割り込み、`arm-cortex-m-bus-fault.robot` の Fault 配送・ロックアップ、例外復帰ファイルのセキュリティ状態や FNC_RETURN を扱うケースです。SysTick の上流仮想時間と同じリロード境界も未検証です。

追加テストは `cargo test renode_` で実行できます。通常の `cargo test` は 66 件成功しました。STM32F072 の公開 ELF/HEX は次の節の手順で 7 テスト・全 8 シナリオを再実行しました。

#### 対応表の回帰を実行する

```sh
renode_root=$(realpath ../renode)
test "$(git -C "$renode_root" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
cargo test
assets=$(mktemp -d)
sed -En 's/^sysbus Load(ELF|HEX) @//p' tests/fixtures/stm32f072-*.resc |
  sort -u |
  while IFS= read -r url; do
    curl -fL "$url" -o "$assets/${url##*/}" || exit 1
  done
PENV_RENODE_ROOT="$renode_root" PENV_RENODE_ASSETS="$assets" \
  cargo test --release upstream_stm32f072 -- --ignored
```

公開イメージによる回帰は 7 テストで、Hello World の ELF/HEX の 2 形式、Console Echo の 2 経路、ADC、Blinky、Button、CRC の計 8 シナリオを確認しました。

Thumb-2 ELF 全体は Cortex-M85 向けです。移植元の命令と現行の実行限界は次の手順で再現できます。Cortex-M0 の最小 REPL での実行は、最初のワイド `MOVS.W`（PC `0x108e`、命令の先頭半ワード `0xf05f`）で停止します。

```sh
cargo test renode_
cargo build --release
thumb_url=$(awk '/^\$\{ELF\}/ {print $NF}' ../renode/tests/unit-tests/tlib/arm/thumb2-data-processing-instr.robot)
curl -fL "$thumb_url" -o "$assets/thumb2-upstream.elf"
llvm-objdump -d --start-address=0x1002 --stop-address=0x100a "$assets/thumb2-upstream.elf"
llvm-objdump -d --start-address=0x1a1e --stop-address=0x1a22 "$assets/thumb2-upstream.elf"
cat >"$assets/thumb2-subset.repl" <<'EOF'
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x40000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
EOF
target/release/penv run --platform "$assets/thumb2-subset.repl" \
  --max-steps 10000 "$assets/thumb2-upstream.elf"
```

## 現在の実装範囲

RV32I 基本整数命令、M 拡張、Zifencei を実装しています。REPL で `rv32ima` または `rv32ima_zicsr_zifencei` を指定すると、共有 RAM 上の A 拡張命令を使用できます。`rv32imc` と `rv32imac` では RV32C の整数圧縮命令に対応します。`cycle`、`instret`、`time` と各上位 32 ビットを読み取れます。CLINT がある場合の `time` は `mtime` に従います。Zicsr の命令と、マシンモードの CSR の一部、および `ECALL`・`EBREAK` からの `MRET` を実装しています。`CPU.RiscV32` ではユーザーモードへの移行とマシンモードへのトラップ、16 エントリーの PMP に対応しています。スーパーバイザーモードでは同期例外の委譲と `SRET`、32 ビット物理アドレス範囲の Sv32 ページ変換に対応しています。変換はハートごとに 64 エントリーをキャッシュし、SFENCE.VMA でそのハートのキャッシュを無効化します。マシンモードから設定したスーパーバイザー割り込みを委譲できます。3 コンテキスト以上の PLIC、その他の ISA 拡張、REPL のその他の構文とデバイス、起動コマンド以外の RESC 構文、ネットワーク、複数ノードは未対応です。標準構成の UART は単一の送信レジスターを提供します。

RV32I の仕様は [RISC-V ISA マニュアル](https://docs.riscv.org/reference/isa/unpriv/rv32.html) を参照してください。
