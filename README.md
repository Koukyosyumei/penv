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

## 現在の実装範囲

RV32I 基本整数命令、M 拡張、Zifencei を実装しています。REPL で `rv32ima` または `rv32ima_zicsr_zifencei` を指定すると、共有 RAM 上の A 拡張命令を使用できます。`rv32imc` と `rv32imac` では RV32C の整数圧縮命令に対応します。`cycle`、`instret`、`time` と各上位 32 ビットを読み取れます。CLINT がある場合の `time` は `mtime` に従います。Zicsr の命令と、マシンモードの CSR の一部、および `ECALL`・`EBREAK` からの `MRET` を実装しています。`CPU.RiscV32` ではユーザーモードへの移行とマシンモードへのトラップ、16 エントリーの PMP に対応しています。スーパーバイザーモードでは同期例外の委譲と `SRET`、32 ビット物理アドレス範囲の Sv32 ページ変換に対応しています。変換はハートごとに 64 エントリーをキャッシュし、SFENCE.VMA でそのハートのキャッシュを無効化します。マシンモードから設定したスーパーバイザー割り込みを委譲できます。3 コンテキスト以上の PLIC、その他の ISA 拡張、REPL のその他の構文とデバイス、起動コマンド以外の RESC 構文、ネットワーク、複数ノードは未対応です。標準構成の UART は単一の送信レジスターを提供します。

RV32I の仕様は [RISC-V ISA マニュアル](https://docs.riscv.org/reference/isa/unpriv/rv32.html) を参照してください。

## 複数マシンと仮想 Ethernet

`penv run --script network.resc --stats` は、RESC で作成したマシンを作成順に 1 ステップずつ進めます。各ラウンドで、送信されたフレームは次のマシンが実行される前にスイッチから配送されます。UART 出力は標準出力にまとめられ、`--stats` はマシンごとの停止理由を表示します。

```text
emulation CreateSwitch "lan"
mach create "sender"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @sender.elf
connector Connect sysbus.eth lan

mach create "receiver"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @receiver.elf
connector Connect sysbus.eth lan
start
```

`mach set` にはマシン名か作成順の番号（0 始まり）を渡せます。`connector Disconnect` でポートを切断でき、`lan EnablePromiscuousMode sysbus.eth` でポートをプロミスキャスモードに変更できます。`emulation RunFor "0.001"` は 1,000 ラウンド実行し、後続の RESC コマンドに制御を戻します。`--max-steps` はスクリプト全体の上限です。実行コマンドを省略すると、スクリプトの設定後に自動で実行します。

対応する REPL デバイスは `Network.LiteX_Ethernet`、`Network.LiteX_Ethernet_CSR32`、STM32F 向け `Network.SynopsysEthernetMAC` です。LiteX では `buffer` 領域をマップし、MDIO が必要なら `phy` 領域と `Network.EthernetPhysicalLayer` を追加します。STM32F の記述子とパケットバッファーはゲスト RAM に置き、MAC の割り込みは NVIC に配線できます。

STM32H7 の `Network.SynopsysDWCEthernetQualityOfService` は MAC、MTL、DMA の各領域と単一の DMA チャネルに対応します。受信・送信リング、Clause 22 MDIO、NVIC への割り込みを利用できます。

STM32H7 の REPL で `RegistrationPeripheralAlias.ETH1MAC` を指定すると、RCC の AHB1 クロックとリセットを MAC に接続します。`RegistrationPeripheralAlias.SYSCFG` は、APB4 クロックの有効化後に PMCR の MII/RMII 選択を MAC に反映します。RCC は実装済みの STM32H7 周辺機器にクロックとリセットを配線します。

STM32H7 参照ボードの PWR と FMC が使う定数応答の `Python.PythonPeripheral` を読み込めます。対応するスクリプトはオフセットごとに定数を返す形式です。Cortex-M 用の RESC では `EnableZephyrMode` が ELF の `z_impl_k_busy_wait` をフックし、待機時間中は仮想時間と SysTick を進めます。抑止シンボルを引数に指定した場合、ELF にそのシンボルがあればフックを設定しません。`VectorTableOffset` は数値またはバッククォート内の `sysbus GetSymbolAddress` を受け付けます。

STM32H7 の ADC は通常・注入シーケンス、周期トリガー、アナログウォッチドッグ、`Analog.Potentiometer` 入力に対応します。ADC の DMA 要求は DMAMUX を経由して STM32 DMA ストリームへ配送し、サンプルを RAM に転送します。

STM32H7 の `DMA.STM32LDMA` は 8 本の BDMA チャネルを提供します。RCC の AHB4 クロック・リセットと NVIC/EXTI への割り込みを配線でき、メモリー間のコピーと ADC サンプルの要求駆動転送に対応します。

## STM32H7 SPI4 と Zephyr 起動検証

`SPI.STM32H7_SPI` はマスター動作、8/16/32 ビットの MMIO、転送完了・サスペンド、IRQ、受信 DMA に対応します。参照ボードの SPI4 は RCC の APB2 クロックとリセットに従い、`DMARecieve -> dmamux1@83` を通じて受信データを DMA に渡します。接続デバイスがない SPI バスの応答はゼロです。外部 SPI デバイス、スレーブ動作、I2S、送信 DMA 要求の生成は未対応です。 [Renode の SPI モデル](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b6a841a013db85/src/Emulator/Peripherals/Peripherals/SPI/STM32H7_SPI.cs)

上流ツリー `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` で検証しました。以下の GCC 指定は、検証環境に `cc` がなかったためです。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
cargo build --release

assets=$(mktemp -d)
renode=../renode
h7="$renode/scripts/single-node/stm32h7_renode_reference_board.resc"
smp="$renode/scripts/single-node/litex_vexriscv_smp.resc"
for url in $(rg --no-filename -o 'https://[^[:space:]]+' "$h7" "$smp"); do
    curl -fL "$url" -o "$assets/${url##*/}"
done
(cd "$assets" && sha1sum zephyr_rrp-demo.elf-*)
# 期待する SHA-1 は c5f12b2aac7af72f7430894558749a984eb7d94e

target/release/penv run --script "$h7" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 1000000 --stats
target/release/penv run --script "$smp" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 400000000 --stats
```

- `cargo test` はユニットテスト 53 件、CLI テスト 5 件が通過しました。STM32H7 の DMA 記述子と LiteX の CSR32 スロットを仮想スイッチで接続し、両方向のフレームと受信データを確認しました。SPI のテストは RCC ゲート、リセット、DMAMUX の要求番号、受信 DMA、NVIC、サスペンドを確認します。
- STM32H7 の上流単一ノード RESC は `aa62fb3` で `stm32h7.repl:193` の `SPI.STM32H7_SPI`、`7d09525` で `stm32h7.repl:199` の `I2C.STM32F7_I2C` により停止しました。`84dc28a` は `stm32h7.repl:219` の `DMA.STM32DMA2D` で停止しました。`eaef188` は `stm32h7.repl:225` の `Video.STM32LTDC` で停止しました。`0de37fd` は `stm32h7.repl:500` のシステムフラッシュ初期化 `init:` で停止しました。`71beca0` は `stm32h7.repl:524` の `Timers.STM32L0_LpTimer`（`lptimer1`）で停止しました。`43e5aa2` は `stm32h7.repl:559` の `Timers.STM32_IndependentWatchdog` で停止しました。`b96599c` は `stm32h7.repl:562` の `Miscellaneous.STM32H7_HardwareSemaphore` で停止しました。`426c3be` は `stm32h7.repl:584` の `timer1:` 追加定義で停止しました。`e189acf` は名前だけの追加定義を読み込み、`stm32h7.repl:726` の `SD.STM32HSDMMC` で停止します。上流複数ノード RESC は `emulation CreateCANHub` で停止します。
- LiteX SMP は同梱指定の OpenSBI と Linux 5.0.9 を実行し、4 CPU の立ち上げと `Run /init as init process` を出力しました。その後、指定の命令上限で停止しました。

双方向 Ethernet の回帰テストは個別に実行できます。STM32H7 の DMA 記述子から LiteX の受信スロットへフレームを送り、逆方向では LiteX の送信スロットから STM32H7 の受信バッファーへ返した内容を照合します。

```sh
cargo test simulation::tests::switches_frames_between_stm32h7_dma_and_litex_slots_in_both_directions -- --exact
```

変更前の停止を同じ RESC で再現するには、基準コミットを一時ディレクトリに展開して実行します。

```sh
baseline=$(mktemp -d)
git archive aa62fb3 | tar -x -C "$baseline"
CARGO_TARGET_DIR="$baseline/target" cargo build --release --manifest-path "$baseline/Cargo.toml"
"$baseline/target/release/penv" run --script "$h7" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 1000000 --stats
```

### 公開 STM32H7 ELF の USART2 スモークテスト

検証スクリプトは同じ上流 RESC のリセットマクロを使い、REPL の参照だけを生成した診断構成に差し替えます。診断構成は上流 REPL から起動に必要な宣言を抽出し、Cortex-M7、480 MHz の SysTick、USART2 の EXTI 配線を使います。DBGMCU ID と CAN CCFG は定数応答です。システムフラッシュは上流の `init:` ブロックを含めて読み込みます。ELF は上流 URL から取得し、SHA-1 を検証します。キャッシュ済みファイルを使う場合は `PENV_ASSET_DIR` を指定できます。

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc tests/stm32h7-zephyr-smoke.sh
```

スクリプトは USART2 の出力と Penv のログを一時ディレクトリに保存します。公開 ELF の起動バナーとシェルプロンプトに加え、実行ステップ 850,000 で入力した `help` への応答、1,300,000 で入力した `kernel version` への応答を検証します。1,700,000 ステップで `sensor get veml7700@10 light` を送ります。実行は 2,600,000 ステップの上限で終了します。

```text
*** Booting STM32H7 Renode Platform Demo 6607bdd711f4 ***
demo:~$ help
Available commands:
demo:~$ kernel version
Zephyr version 4.4.99
demo:~$ sensor get veml7700@10 light
channel type=18(light) … (499.000000)
```

変更前の診断実行で停止した `pc=0x080036b6` は、ITCM を初期化する `strd r0, r1, [r3], #8`（先頭半語 `0xe8e3`）でした。Thumb-2 のデコーダーはローカル nRF52840 担当の `fe1911b` を再利用し、Cortex-M7 の実行ログで必要になった命令を追加しました。CRC の演算モデルはローカル STM32F0 の実装を再利用しています。起動時のバスアクセスに合わせて、RCC、SysTick、RNG、TIM3、CRC、QuadSPI の初期化・識別・SFDP と受信送信割り込み、FDCAN の設定レジスターを配線しました。

残課題は未変更の上流 REPL のロード、VEML7700 以外の I²C センサー、QuadSPI のフラッシュデータ操作、CAN フレーム送受信です。診断実行では LSM6DSO の初期化失敗と外部フラッシュのマウント失敗 `err=-45` が出力されます。MPU とキャッシュはファームウェアの初期化レジスターに応答する範囲です。

### VEML7700 のデータ転送と I²C IRQ

診断構成は上流参照ボードの `Sensors.VEML7700 @ i2c1 0x10` を取り込み、静的な `Illuminance: 500` を指定します。公開デモ ELF の Zephyr ドライバーが設定とレジスター番号を書き、反復 START で ALS データを読みます。STM32 のイベント割り込みは I²C1 から EXTI 22 を経由して NVIC IRQ 31 へ届きます。500 lux 指定では UART が `499.000000`、1,000 lux 指定では `999.000000` を返しました。 [Renode の VEML7700 モデル](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/Sensors/VEML7700.cs)

次の比較は、スモークテストが表示した一時ディレクトリを `work` に、公開 ELF の保存先を `assets` に設定して実行します。後半は起動バナーを確認できる時点で、ゲストの `devmem` から NVIC の IRQ 31 を無効にします。同じセンサーコマンドが `Read failed` を返すことで、データ読み出しの完了が I²C1 イベント IRQ に依存することを確認できます。両方とも実行上限で終了します。

```sh
sed 's/Illuminance: 500/Illuminance: 1000/' "$work/boot.repl" > "$work/light1000.repl"
sed 's/@boot.repl/@light1000.repl/' "$work/boot.resc" > "$work/light1000.resc"
target/release/penv run --script "$work/light1000.resc" \
    --platform-root ../renode --asset-dir "$assets" --max-steps 3500000 \
    --uart-event "1700000:$work/sensor.txt"

printf 'devmem 0xe000e180 32 0x80000000\r' > "$work/mask.txt"
target/release/penv run --script "$work/boot.resc" \
    --platform-root ../renode --asset-dir "$assets" --max-steps 3500000 \
    --uart-event "1400000:$work/mask.txt" --uart-event "1700000:$work/sensor.txt"
```

I²C DMA、スレーブ動作、VEML7700 の時系列入力は未対応です。別の上流 Nucleo H753ZI light polling ELF は診断構成で起動しましたが、非ゼロ測定値は確認できていません。

### DMA2D の転送と IRQ

`DMA.STM32DMA2D` は 32 ビット MMIO、AHB3 のクロック・リセット、行オフセット付きの RAM コピーと単色塗りつぶし、完了・エラーフラグと NVIC IRQ に対応します。参照ボードの宣言は `0x52001000` と NVIC IRQ 90 を使います。ピクセル形式変換、ブレンディング、CLUT、転送中の停止は未対応です。 [Renode の DMA2D モデル](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/DMA/STM32DMA2D.cs)

USART2 スモークテストは上流の DMA2D 宣言を含む診断用 REPL で、Zephyr Shell と VEML7700 の読み出しを確認します。そのスクリプトの一時ディレクトリを `work` に、公開 ELF の保存先を `assets` に設定すると、同じ公開 ELF の Shell から DMA2D を試せます。

```bash
sed 's/@boot.repl/@fill.repl/' "$work/boot.resc" > "$work/fill.resc"
cp "$work/boot.repl" "$work/fill.repl"
printf '\ndma2dScratch: Memory.MappedMemory @ sysbus 0x60000000\n    size: 0x1000\n' >> "$work/fill.repl"

events=()
step=1000000
for command in '0x580244d4 32 0x10' '0x52001038 32 0x1234abcd' \
    '0x5200103c 32 0x60000000' '0x52001044 32 0x00010001' \
    '0x52001000 32 0x00030201' '0x52001004 32' '0x60000000 32' '0xe000e208 32'; do
    printf 'devmem %s\r' "$command" > "$work/dma-$step.txt"
    events+=(--uart-event "$step:$work/dma-$step.txt")
    step=$((step + 200000))
done
target/release/penv run --script "$work/fill.resc" \
    --platform-root ../renode --asset-dir "$assets" --max-steps 2800000 "${events[@]}"
```

読み戻しは順に DMA2D の完了フラグ `0x2`、RAM のピクセル値 `0x1234abcd`、NVIC の保留ビット `0x4000000` でした。ユニットテストでは IRQ 90 の保留と、完了フラグをクリアした後に再保留しないことも確認しています。

### LTDC のフレームバッファ読み取り

`Video.STM32LTDC` は `0x50001000` の 32 ビット MMIO、APB3 のクロック・リセット、即時シャドーレジスター再読み込みと NVIC IRQ 88 の配線に対応します。`--framebuffer-out PATH` は単一マシンの実行終了時に、有効化して再読み込み済みのレイヤー 1 を P6 RGB PPM に保存します。ゲスト RAM から、CFBLL − 7 バイトの各行を CFBP バイト間隔で CFBLNBR 行読み、そのままレイヤーの画像寸法に使います。ARGB8888、RGB888、RGB565 に対応します。 [Renode の LTDC モデル](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/Video/STM32LTDC.cs)

前節の DMA2D 手順で作った `events` と `step` に、以下の LTDC 設定を追加します。同じ公開 Zephyr ELF の Shell が DMA2D で RAM にピクセルを書き、LTDC が同じアドレスから読み出します。

```bash
for command in '0x580244e4 32 0x8' '0x500010ac 32 0x60000000' \
    '0x500010b0 32 0x0004000b' '0x500010b4 32 1' '0x50001084 32 1' \
    '0x50001024 32 1' '0x50001018 32 1'; do
    printf 'devmem %s\r' "$command" > "$work/ltdc-$step.txt"
    events+=(--uart-event "$step:$work/ltdc-$step.txt")
    step=$((step + 200000))
done
target/release/penv run --script "$work/fill.resc" \
    --platform-root ../renode --asset-dir "$assets" --max-steps "$step" \
    "${events[@]}" --framebuffer-out "$work/frame.ppm"
printf 'P6\n1 1\n255\n\x34\xab\xcd' | cmp - "$work/frame.ppm"
```

公開 Zephyr ELF の Shell と VEML7700 スモークテストも通過しました。上流 `STM32LTDC.robot` が指定する STM32H747I display ELF は、USART1 と SDRAM を追加した診断構成で試し、`pc=0x080031a2` の `0x5802648c` への未マップアクセスで停止しました。専用デモの表示、レイヤー合成、CLUT、走査タイミング、垂直帰線時の再読み込み、FIFO アンダーランは未検証または未実装です。

### システムフラッシュの REPL 初期化

メモリの `init:` では、領域先頭からのオフセットを指定する `WriteWord` と `WriteDoubleWord` を読み込み時に適用します。上流 STM32H7 の UID 生成式は、アドレスとオフセットから得た再現可能な値で処理します。同じ配置のマシン同士では UID も同じ値になります。任意の Python 式やその他の初期化コマンドは未対応です。

USART2 スモークテストが表示した診断用ディレクトリを `work`、公開 ELF の保存先を `assets` に設定して実行します。

```bash
events=()
step=1000000
for command in '0x1ff1e880 16' '0x1ff1e800 32' '0x1ff1e804 32' '0x1ff1e808 32'; do
    printf 'devmem %s\r' "$command" > "$work/flash-$step.txt"
    events+=(--uart-event "$step:$work/flash-$step.txt")
    step=$((step + 200000))
done
target/release/penv run --script "$work/boot.resc" \
    --platform-root ../renode --asset-dir "$assets" --max-steps "$step" "${events[@]}"
```

公開 Zephyr ELF の Shell から、容量ワード `0x800` と UID の `0x08e827a7`、`0x175bcde3`、`0x6ae78d93` を確認しました。同じ初期化を含む診断構成で、VEML7700 の `499.000000` lux と、DMA2D から LTDC に渡した PPM の RGB 値 `34 ab cd` も再確認しました。未変更の上流 RESC の次の停止箇所は `SD.STM32HSDMMC` です。

### LPTIM の比較イベントと割り込み

`Timers.STM32L0_LpTimer` は内部クロックによる単発・連続カウント、プリスケーラー、比較・自動再ロードイベント、ステータスのクリアに対応します。上流 STM32H7 REPL の LPTIM1〜5 は RCC の APB1L／APB4 と EXTI に配線します。外部クロック・トリガー、エンコーダー、波形出力、プリロード、繰り返し回数によるイベント制御は未対応です。 [Renode の LPTIM モデル](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/Timers/STM32L0_LpTimer.cs)

USART2 スモークテストが表示した診断用ディレクトリを `work`、公開 ELF の保存先を `assets` に設定して実行します。

```bash
events=()
step=1000000
for command in '0x580244e8 32 0x200' '0x40002408 32 1' '0x40002410 32 1' \
    '0x40002414 16 64' '0x40002418 16 128' '0x40002404 32 0xffffffff' \
    '0x40002410 32 3' '0x40002400 32' '0xe000e208 32' \
    '0x40002404 32 3' '0xe000e288 32 0x20000000' '0x40002400 32' '0xe000e208 32'; do
    printf 'devmem %s\r' "$command" > "$work/lp-$step.txt"
    events+=(--uart-event "$step:$work/lp-$step.txt")
    step=$((step + 200000))
done
target/release/penv run --script "$work/boot.resc" \
    --platform-root ../renode --asset-dir "$assets" --max-steps "$step" "${events[@]}"
```

公開 Zephyr Shell は LPTIM1 の比較・自動再ロードフラグ `0x3` と、EXTI 47 を経由する NVIC IRQ 93 の保留ビット `0x20000000` を読み戻しました。フラグと保留をクリアした後は両方とも `0x0` でした。ユニットテストでは、この経路でゲストのハンドラーへ分岐して先頭命令を実行することも確認しています。VEML7700 の読み出し、システムフラッシュ、DMA2D から LTDC への PPM 出力も再確認しました。

### 独立ウォッチドッグの給餌とリセット

`Timers.STM32_IndependentWatchdog` は、キーによる書き込み保護、プリスケーラー、リロード、ウィンドウ範囲外の早期給餌、期限切れによるリセットに対応します。リセット時に Cortex-M の PC と SP をベクターテーブルから取り直し、NVIC、SysTick、USART、IWDG と対応する STM32H7 周辺機器の状態を初期化します。SRAM とロード済みのイメージは保持します。 [Renode の IWDG モデル](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/Timers/STM32_IndependentWatchdog.cs)

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc tests/stm32h7-zephyr-smoke.sh
```

スクリプトは公開 STM32H7 ELF を診断構成で起動し、Shell と VEML7700 の回帰に続いて IWDG を確認します。32 kHz、分周比 256、リロード値 `0x7f` で約 1.024 秒の期限を設定し、約 1.6 秒で開始、約 2.2 秒で給餌します。当初の期限を過ぎた約 2.8 秒で Shell が応答し、その後は給餌を止めると起動バナーが再表示され、約 4.3 秒の Shell コマンドにも応答します。検証ログは `watchdog-uart.log` と `watchdog-penv.log` です。

上流の `tests/platforms/STM32L072.robot` にある `Independent Watchdog Should Trigger Reset` の条件を移植しました。テストでは ELF の再ロードに相当する処理を介さず、リセット後のゲスト再進入と SRAM 保持を検証します。その Robot テストが指定する公開 STM32L072 ELF の試行は、簡易 STM32L0 REPL の `0xe000ed94` へのアクセスで停止しました。RCC のリセット原因フラグ、デバッグ時停止、すべての周辺機器を含むボード全体のリセットは残課題です。 [上流 Robot テスト](https://github.com/renode/renode/blob/master/tests/platforms/STM32L072.robot)

### ハードウェアセマフォのロックと解放通知

`Miscellaneous.STM32H7_HardwareSemaphore` は、32 個の書き込みロックと Cortex-M7 の読み取りロック、所有者照合による解放、キー付き一括解放、CPU1 の解放ステータスと通知マスク、AHB4 のクロックとリセットに対応します。上流 STM32H7 REPL の宣言は IRQ が未接続のため、診断用 REPL では STM32H753 の NVIC IRQ 125 に配線します。CPU2 の通知バンクとデュアルコア実行は未対応です。 [Renode の HSEM モデル](https://github.com/renode/renode-infrastructure/blob/master/src/Emulator/Peripherals/Peripherals/Miscellaneous/STM32H7_HardwareSemaphore.cs) [STM32H753 CMSIS 定義](https://github.com/STMicroelectronics/cmsis-device-h7/blob/master/Include/stm32h753xx.h)

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc tests/stm32h7-zephyr-smoke.sh
```

HSEM の検証ログは `hsem-uart.log` と `hsem-penv.log` です。公開 ELF の Zephyr Shell からセマフォ 3 を確保し、異なるプロセス ID での解放後も `0x8000032a` のまま、解放ステータスは `0x0` であることを確認します。正しい所有者で解放すると CPU1 のマスク済みステータスが `0x8`、NVIC 保留レジスターが `0x20000000` になります。クリア後は両方とも `0x0` となり、セマフォ 4 の読み取りロックは `0x80000300` を返します。Shell、VEML7700、ウォッチドッグの回帰も同じスクリプトで確認します。

未変更の上流 RESC は HSEM と `timer1:` の追加定義を通過し、`SD.STM32HSDMMC` で停止します。

### 名前付き REPL への追記

すでに宣言された名前を型や登録先なしで記述すると、以前の登録を引き継いでプロパティを上書きし、配線行を追加します。`using` で読み込んだ定義にも追記できます。型付きの再宣言は従来どおり重複として診断し、存在しない対象への追記もエラーになります。登録先の差し替え、既存配線を取り除く `none`、`init add:` は未対応です。 [Renode の REPL 仕様](https://renode.readthedocs.io/en/latest/advanced/platform_description_format.html)

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/stm32h7-zephyr-smoke.sh

# スモークテストと同じ ELF 保存先を指定
target/release/penv run --script ../renode/scripts/single-node/stm32h7_renode_reference_board.resc \
    --platform-root ../renode --asset-dir "$assets" --max-steps 1000000
```

診断構成は上流ボードの `usart2:`、`watchdog:`、`rcc:` の追記を使い、公開 ELF で Shell、VEML7700、ウォッチドッグの再起動、HSEM 通知を再確認しました。単体テストでは、インクルードをまたぐプロパティの上書きと EXTI 配線の追加を確認しています。未変更の上流 RESC の次の停止箇所は `SD.STM32HSDMMC` です。上流の timer/GPIO 代替機能配線は REPL に取り込まれますが、代替機能の実行と未対応のタイマー種別は今後の対応です。

### STM32H7 の SDMMC、USB、sysbus タグ

`SD.STM32HSDMMC` はカードなしで発行されたコマンドに `CTIMEOUT` を返し、割り込みマスクとクリアを NVIC IRQ 49 に反映します。`USB.DesignWare_USB_OTG` はホスト未接続時のコア識別、アイドル状態、構成レジスターを提供します。REPL の `sysbus: init: Tag` は未マップアクセスの読み戻し値を設定します。範囲が重なる場合は後のタグが優先され、通常の RAM と MMIO はタグより優先されます。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
target/release/penv run \
    --script ../renode/scripts/single-node/stm32h7_renode_reference_board.resc \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1000000
```

公開 Zephyr の診断構成で Shell、VEML7700 の 499 lux、IWDG の再起動、HSEM 通知を確認しました。ゲストの `devmem` では SDMMC の `CTIMEOUT=0x4` と NVIC 保留 `0x20000` がクリア後にともに 0 になり、CCFG タグは `0x4`、タグと重なる USB の実レジスターは `0x4f54310a` を返しました。

`c3c56a4` では未変更の RESC は `platforms/cpus/stm32h753.repl:3` の `Miscellaneous.Crypto.STM32H7_CRYPTO` で停止します。SD カード接続、転送と IDMA、上流 `STM32SDMMC.robot` の ext2 マウント、USB ホスト接続とパケット転送は残件です。上流の専用 STM32H747I display ELF は、USART1 と SDRAM を追加した診断構成で従来の HSEM アクセスを通過し、`HAL_DSI_Init` の `pc=0x08004576` で `0x50000430` の未マップアクセスに到達しました。

### h7-continuation の追加検証

STM32H753 の `Miscellaneous.Crypto.STM32H7_CRYPTO` は AES-ECB の 128、192、256 ビット鍵、暗号化・復号、FIFO と NVIC IRQ 79 のマスクに対応します。AES の実装には RustCrypto の `aes` を使用します。上記スモークテストでは公開 Zephyr Shell を通して AES-128 の暗号文 `69c4e0d8 6a7b0430 d8cdb780 70b4c55a` と出力 FIFO の割り込みステータスを照合します。`OutDMA` 配線は構文を検証し、DMA 転送と ECB 以外の暗号モードは残件です。

上流 `STM32LTDC.robot` のシャドーレジスター検証を同じスモークに移植しました。DMA2D の完了と IRQ 90 を確認してから LTDC にピクセルを渡し、行数レジスターを 0 に変更しても、再読み込みまで有効な 1×1 PPM が RGB `34 ab cd` になることを照合します。専用 ELF でのボタン入力と FIFO アンダーランの検証は残件です。

上流 `STM32SDMMC.robot` の ELF と ext2 イメージは取得し、SHA-1 を照合しました。USART1 と SDRAM を追加した診断構成で、ファイルシステム用 ELF は `stm32h7_m4_wakeup` の `pc=0x080011dc` で `0x58000704` の未マップアクセスに到達しました。カードを接続する RESC コマンドと、マウント・ファイル一覧の移植も残っています。

`cargo test` はユニット 56 件と CLI 5 件が通過しました。個別の STM32H7 DMA と LiteX CSR32 の双方向フレーム交換も通過し、上流 LiteX SMP の実イメージは 4 CPU の起動と `Run /init as init process` に到達しました。未変更の STM32H7 RESC は CRYP と RGB LED の宣言を通過し、`platforms/boards/stm32h7_renode_reference_board.repl:9` の `pwmled0: Miscellaneous.PWMLED` で停止します。公開 Zephyr Shell から GPIOD 13 をセットし、`--stats` で `redRgbLed=on`、`greenRgbLed=off`、`blueRgbLed=off` を確認しました。PWM のデューティ比、追加 I²C センサー、タイマー代替機能配線、未変更構成でのファームウェア起動と実ゲスト間 Ethernet は残件です。

### STM32H747I の display と SD ファイルシステム ELF

上流 Robot が指定する 2 本の ELF を使います。スモークテストが公開 URL から取得し、SHA-1 を照合します。第 2 スクリプトの引数には、第 1 スクリプトが出力するログディレクトリを指定します。診断 REPL に USART1、外部 SDRAM、DSI ホストを追加します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
bash tests/stm32h7-display-sd-smoke.sh /tmp/第1スクリプトのログディレクトリ
```

- **Display**。DSI ラッパーはレギュレーターの準備完了、PLL のロック、PHY のレーン停止状態を制御レジスターに応じて返します。実 ELF で DSI ホスト初期化を通過し、UART に `Display starts` と `Display updated` が出ました。LTDC の行長を H7 HAL に合わせて `CFBLL − 7` として解釈し、複数色を含む 800×480 RGB PPM を検証しました。NT35510 のパネル ID 読み取り、ボタン入力、FIFO アンダーランは残件です。 [STM32H747 CMSIS](https://github.com/STMicroelectronics/cmsis_device_h7/blob/master/Include/stm32h747xx.h) [STM32H7 LTDC HAL](https://github.com/STMicroelectronics/stm32h7xx-hal-driver/blob/master/Src/stm32h7xx_hal_ltdc.c)
- **SD**。単一 Cortex-M7 構成で `SYSCFG_UR1.BCM4` は 0 を返し、`RCC_GCR.BOOT_C2` への要求を保持します。SDMMC は応答不要の `CMD0` に送信完了を返し、応答を要求するコマンドにはカード未接続時にタイムアウトを返します。実 ELF は Zephyr を起動し、UART に `Error mounting disk.` を出しました。Cortex-M4 の実行、SD イメージ接続、カード応答と転送、ext2 マウントは残件です。
- **回帰**。ユニット 57 件と CLI 5 件、公開 STM32H7 Shell、VEML7700、DMA2D から LTDC への 1×1 PPM とシャドーレジスターの検証が通過しました。

### close-h7-platform

前節の両スモークを実行すると、公開 Zephyr の診断 Shell と、上流 Robot が指定する display・SD の ELF を検証できます。

- **Display**。診断 REPL の DSI ホストに `panel: "NT35510"` を設定します。DCS の Read ID2 を受けたときにパネル ID を読み出し FIFO に入れます。Zephyr は `nt35510: Init complete(0)` と `Display updated` を出力し、800×480 の多色 PPM を取得できました。LTDC のボタン入力とアンダーラン、一般の DSI パネル応答とリンクタイミングは残件です。 [Zephyr NT35510 ドライバー](https://github.com/zephyrproject-rtos/zephyr/blob/v4.3.0/drivers/display/display_nt35510.c)
- **SD**。`--sd-image PATH` で単一の STM32H7 SDMMC に、サイズが 512 KiB の倍数の読み取り専用 SDHC イメージを接続します。SD 初期化、アドレス選択、ブロック読み取り、受信 FIFO と IRQ に対応します。上流 `STM32SDMMC.robot` のイメージで `Disk mounted.` と `[DIR ] lost+found` を確認し、カードなしの場合のエラー経路も検証しました。M4 起動要求は単一 M7 構成で保持されます。M4 の命令実行、カードへの書き込みと IDMA は残件です。
- **Shell と上流定義**。診断 Shell で既存の VEML7700 と新しい IIS2MDC を読み出し、TIM3 CH1 → GPIOB4 AF2 の PWM を `pwmled0=25.00%` として確認しました。PWM のレベル表示はエッジ整列のアップカウント設定に対応し、IIS2MDC は静止したゼロ磁場を返します。未変更 RESC の最初の停止箇所は `stm32h7_renode_reference_board.repl:50` の `Sensors.TMP108` です。追加の I²C センサー、上流タイマーの IRQ と代替機能の全配線、未変更構成での Shell 起動は残件です。

### close-h7-io

上流 Robot の観測条件を下記のスモークに移植しました。第 2・第 3 スクリプトには、第 1 スクリプトが表示したディレクトリを渡します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
bash tests/stm32h7-display-sd-smoke.sh /tmp/第1スクリプトのログディレクトリ
bash tests/stm32h7-io-smoke.sh /tmp/第1スクリプトのログディレクトリ
```

| 対象 | 実 ELF での確認と残件 |
| --- | --- |
| Shell、VEML7700 | 公開 STM32H7 参照デモで UART の `help` と `kernel version` に応答。上流 `VEML7700.robot` の Shell ケースから 500・1000 lux の条件を移植し、499・999 lux を照合。照度は実行ごとに REPL から設定。 |
| IIS2MDC | 上流 `IIS2MDC.robot` の正・負のベクトルを参照 Shell で照合。REPL の `MagneticFluxDensityX/Y/Z` は整数 nT。上流専用 polling ELF でも同じ正・負のベクトルを別々の実行で照合。RESD の時系列と GPIO トリガーは残件。 |
| DMA2D、LTDC | Shell から DMA2D の完了・ピクセル・IRQ 90 を照合。上流 `STM32LTDC.robot` のゼロ行テストを移植し、設定をステージした時点では ISR が 0、即時再読み込み後は ISR が `0xa`、IRQ 88 が保留。復元後に元の PPM を取得。専用 display ELF のパネル初期化と画像出力も通過。ボタン入力と走査に同期した発生タイミングは残件。 |
| PWM | 参照ボードの TIM3 CH1、GPIOB4 AF2、`pwmled0` の配線を使い、実 Shell で 25% を確認。出力はタイマー設定から算出したデューティ比。波形による測定は残件。 |
| SD | 上流 `STM32SDMMC.robot` と同じ ELF・ext2 イメージで `Disk mounted.` と `[DIR ] lost+found` を確認。カード未接続時の失敗経路も照合。 |
| 仮想 Ethernet | 上流 `nucleo_h753zi.robot` の `Should Talk Over Ethernet` と同じ echo server/client ELF を仮想スイッチに接続。診断 REPL で両ノードが起動し、サーバーで TCP 接続受理と 4 回以上の `Received and replied` を照合。クライアント側 `Received and compared … all ok` は残件。STM32H7 と LiteX の双方向フレームは既存のモデルテストで照合。 |

Ethernet の第 3 スクリプトは、`--uart-log-dir` でノード作成順に `0.uart.log` と `1.uart.log` を保存します。ディレクトリは事前に作成し、同名ファイルのない場所を指定します。echo ELF の実行で到達した Cortex-M の `CPSIE/CPSID if` と、LR を使うワード版の排他アクセスにも対応しました。

### accept-network の再監査

上記の 3 本の実ファームウェアスモークと `cargo test` を再実行しました。ユニット 59 件と CLI 5 件が通過しました。IIS2MDC 専用 polling ELF は、START より前に TXDR へ書いたレジスター番号を保持する修正と、Thumb-2 `ADR.W` の実装により起動しました。正ベクトル `0.150000 0.300000 0.001500` と負ベクトル `-1.234500 0.678000 0.000000` を UART から照合しています。

上流ケースの合格判定には、IIS2MDC の RESD 再生と GPIO トリガー、専用 display ELF のボタン入力と走査タイミング、PWM 波形、Ethernet クライアントのペイロード照合が残っています。Ethernet サーバーの TCP 応答は複数回確認できましたが、クライアントの `Received and compared … all ok` は出力されていません。

### close-h7-acceptance-gaps

前節の 3 本のスモークを実行します。`stm32h7-io-smoke.sh` は対象の公開 ELF を取得し、指定された SHA-1 と照合します。各スモークが出力するディレクトリに、UART・RESC・レジスター読み出しのログが残ります。

| 対象ケース | 合格条件と証拠 |
| --- | --- |
| Ethernet クライアントのペイロード照合 | 上流 echo client ELF の `process_tcp` が受信終端まで `memcmp` に成功すると増やす `conf + 0x24` を読み出す。10 仮想秒で `0 → 0x2c` を確認。サーバーの接続受理と 4 回以上の TCP 応答を UART でも照合する。`io-run/echo.log` と `io-run/uart/`。 |
| IIS2MDC の時系列 polling と GPIO trigger | 上流 CSV を 3 仮想秒のオフセット付きで REPL の `SampleCsv` に渡す。両方の上流 ELF で 9 ベクトルの順序と値を照合し、trigger は GPIOG11 の DRDY で駆動する。`io-run/timed/`。 |
| display のボタンと走査タイミング | 専用 ELF で GPIOA0 の押下・解放ありでは `Display updated` が 2 回、同一時間の対照では 1 回。1 FPS で、ゼロ行のステージ中は ISR=0、再読み込み直後と次フレーム直前は `0x8`、フレーム境界後は `0xa`、復元後は 0。`display-sd-run/button/` と `display-sd-run/control/`。 |
| PWM 波形 | STM32H7 参照 ELF の起動後、TIM3 CH1 を 10 ms 周期・25% に設定。GPIOB4 AF2 の入力を 2.4・2.6・9.9・10.1 ms に読み、高・低・低・高を照合。`io-run/pwm/waveform.log`。 |

Ethernet はハッシュ固定の ELF 内の成功カウンターを合格判定に使います。クライアント UART の比較成功行は未確認です。IIS2MDC の入力形式は元の CSV で、RESD コンテナーの読込みは残件です。LTDC はフレーム境界をモデル化しており、画素クロック単位の走査や他の FIFO アンダーラン条件は残件です。PWM 波形はエッジ整列のアップカウントと検証した配線の範囲です。未変更の上流参照 RESC 自体は引き続き TMP108 未対応で停止します。

### full-upstream-network

[上流ケース台帳](tests/stm32h7-full-upstream.tsv) は固定 Renode とサブモジュールの初回棚卸しです。Nucleo H753ZI の機種固有 Robot スイートは全 25 ケースを含みます。各行はリポジトリ、ファイル、行範囲、ケース名、パラメータ参照、判定、判定根拠を記録します。共有 NUnit の DMA 転送マトリクスは、入出力の格納先とアクセス幅の全組み合わせを個別行に展開しました。

- Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`
- Infrastructure `09ac7c563095c5410258ecc1b1a6b841a013db85`
- Infrastructure の tlib `c04522a9c886ba743c77a86b0c879103a2ef46d0`
- Packet.Net `20e0f2fe40743fed4e6a81bb8fe9523c257f168b`
- UI `5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f`

初回棚卸しの 984 行のうち、適用範囲の確認待ち `REVIEW` が 704、未達 `OPEN` が 82、部分移植 `PARTIAL` が 7、適用外の根拠を確認した `OUT` が 191 です。初回台帳は完全な適用性判定やパラメータ展開を終えていません。機種固有の未実装・失敗・ビルド依存を除外理由にはしていません。上流と等価な合格はまだ記録していません。既存の `network-ready` は診断スモークのゲートです。全上流ケース用の `network-full-upstream-ready` は未作成です。

固定の Infrastructure と Packet.Net は作業ディレクトリの外で展開し、`../renode` は読み取り専用の参照にしました。tlib と UI の固定アーカイブに Robot/NUnit のテストファイルは見つかりませんでした。対象 NUnit プロジェクトは .NET 10 を指定しており、今回の環境では `dotnet` と Renode/Robot ランナーを検出できませんでした。直接実行の確認は残件です。

```bash
scan=$(mktemp -d)
curl -fsSL https://codeload.github.com/renode/renode-infrastructure/tar.gz/09ac7c563095c5410258ecc1b1a6b841a013db85 |
    tar -xz -C "$scan" --strip-components=1
mkdir "$scan/packetnet"
curl -fsSL https://codeload.github.com/antmicro/Packet.Net/tar.gz/20e0f2fe40743fed4e6a81bb8fe9523c257f168b |
    tar -xz -C "$scan/packetnet" --strip-components=1
python3 tests/tools/inventory-stm32h7.py ../renode "$scan" "$scan/inventory.tsv"
cmp tests/stm32h7-full-upstream.tsv "$scan/inventory.tsv"
```

TMP108 は固定温度、ビッグエンディアンの温度・閾値、ワンショットからの復帰を実装しました。診断 Shell で `sensor get tmp108@48 ambient_temp` が `23.500000` を返します。時系列入力、比較器と IRQ は残件です。未変更の上流参照 RESC は TMP108 を通過し、`platforms/boards/stm32h7_renode_reference_board.repl:52` の `Sensors.LPS25HB` で停止します。実行コマンドは前節のスモーク 3 本と、SDMMC・USB・sysbus タグ節の未変更 RESC 用コマンドを使います。

### full-h7-triage-and-lps25hb

固定版の [ケース台帳](tests/stm32h7-full-upstream.tsv) で、機種固有の Nucleo H753ZI Robot は全 25 ケースを適用対象に維持しました。既存の 984 行の内訳は、適用対象 778 行（`OPEN` 768、`PARTIAL` 10）、適用外 206 行です。既存行の `REVIEW` は 0 です。この分母は現台帳の行単位で、上流全体の追加発見や未展開のパラメータは含みません。汎用 Cortex-M・REPL・モニター・DMA・バス・パケットの共有契約は対象側に残しました。新たな適用外の行には Zynq 固定 Linux ゲスト、NXP K6xF 固定 ELF、Cortex-M33/TrustZone、Windows 固有操作、PPP または Linux cooked キャプチャ形式の根拠を記録しました。性能ケースは対象に残し、実行は後工程です。

LPS25HB はサブアドレスと自動インクリメント、`WHO_AM_I=0xbd`、設定レジスター、温度と圧力の換算を実装しました。前節の Shell スモークが作る診断 REPL を使い、`stm32h7-io-smoke.sh` が `LPS25HB.robot` 指定の Zephyr 4.2・4.4 ELF を SHA-1 照合して実行します。実行ログはスモークが出力するディレクトリの `pressure/` 配下に残ります。

| 上流ケース | 上流期待値との照合と残件 |
| --- | --- |
| Should Read Temperature And Pressure | v4.2 ELF で初期値、100 °C、−10 °C の UART と既知の 101.325000 kPa を個別に照合。範囲外入力の保持は単体テストで確認。1 回の実行内の入力変更と順序の検証は残件。 |
| Should Correctly Calculate Pressure | 1023.346923828125 hPa が `8d f5 3f` になることを I²C サブアドレス読み出しで照合。Robot のプロパティ getter と直接 Read の再現は残件。 |
| Should Read Samples From RESD | 未達。再生、サンプル切替、終端後に静的値へ戻る処理が必要。 |
| Should Work After Errata Driver Update | v4.4 の同一 ELF が `temp 100.31 Cel, pressure 101.325000 kPa` を出力。元の REPL と実行時入力の等価性は未確定。 |

未変更の上流 RESC は `lps25hb` を通過し、ボード REPL の 54 行目にある `Sensors.LSM6DSO_IMU` で停止します。診断スモークの合格を全上流ケースの合格数には算入していません。

### full-h7-lsm6dso

`Sensors.LSM6DSO_IMU` の I²C ユーザーバンク、加速度・角速度のフルスケールと ODR、DRDY と `Interrupt1` の GPIO 配線に対応しました。RESC では上流と同じ `DefaultAccelerationX/Y/Z` と `DefaultAngularRateX/Y/Z` を設定できます。ストリームの移植は `FeedSamplesFromCsv` で上流の CSV を 12 Hz、開始オフセット 100 ms で再生します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
bash tests/stm32h7-lsm6dso-smoke.sh /tmp/Shellスモークのログディレクトリ
```

スモークは固定上流 `LSM6DSO_IMU.robot` の ELF 3 本を SHA-1 照合し、5 シナリオの UART を Robot と同じ数値・順序で照合します。

| 上流ケース | ログ | 照合 |
| --- | --- | --- |
| I2C polling | `poll-basic/0.uart.log` | ゼロ値のあと、加速度 1/−1/0 g と角速度 100/200/300 dps を順に設定 |
| RESD polling | `poll-stream/0.uart.log` | 同じ CSV の 3 区間と終端後のゼロ復帰 |
| I2C interrupt | `irq-basic/0.uart.log` | GPIOC13 の DRDY を有効にした trigger ELF で polling と同じ刺激 |
| RESD interrupt | `irq-stream/0.uart.log` | trigger ELF で同じ 3 区間と終端後のゼロ復帰 |
| Driver errata | `errata/0.uart.log` | Zephyr 4.4 polling ELF の起動と加速度の期待値 |

いずれも診断 REPL と固定の仮想実行区間を使うため台帳は部分移植です。上流 REPL での実行、Robot の各 UART 待機タイムアウト、RESD コンテナー、FIFO と追加バンクは残件です。未変更の参照 RESC は IMU 定義を通過し、`platforms/cpus/stm32h7.repl:267` の TIM17 に対して `expected an APB1L STM32 timer RCC alias` を返します。対象分母 778 行は維持し、内訳は未達 763、部分移植 15 行です。

### full-h7-tim17

固定上流 REPL の TIM17 宣言を、APB2ENR のビット 18 と APB2RSTR のビット 18 に接続しました。同じ APB2 バンクの TIM15 と TIM16 にも各ビット 16 と 17 を割り当てました。APB1L の既存タイマーは従来のゲートを使います。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
bash tests/stm32h7-io-smoke.sh /tmp/Shellスモークのログディレクトリ
bash tests/stm32h7-lsm6dso-smoke.sh /tmp/Shellスモークのログディレクトリ
target/release/penv run \
    --script ../renode/scripts/single-node/stm32h7_renode_reference_board.resc \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1000000
```

公開参照 ELF の Shell で TIM17 の ARR を読み、クロック停止中と APB1LENR の同ビットを有効にした場合の両方で 0 を確認しました。APB2ENR を有効にすると ARR は `0xffff` を返し、ゲストが書いた `0x42` も読み出せました。EGR で発行した更新イベントで SR の UIF と NVIC IRQ 118 の保留を読み出し、APB2RSTR でリセット後に ARR が初期値へ戻り、IRQ が再保留されないことを確認しました。ログは `tim17-uart.log` と `tim17-penv.log` です。TIM15 と TIM16 は同じバンクへの宣言取り込みまで確認しました。

未変更の上流 RESC は TIM15・TIM16・TIM17 を通過し、`platforms/cpus/stm32h7.repl:305` の `timer8: unsupported STM32 timer RCC alias TIM8` で停止します。TIM8 の APB2 結合と複数 IRQ ルートは残件です。このゲスト検証は診断スモークのため、上流ケース台帳の等価合格数には算入しません。

### scope-audit-network

固定版から現台帳を再生成し、**対象 785 実行構成**を確定しました。適用外 426 構成を含む総数は 1,211 です。機種固有の Nucleo H753ZI は 25 ケースすべて対象です。

| 判定 | 実行構成数 |
| --- | ---: |
| 上流等価の合格 | 0 |
| 部分移植 | 15 |
| 未達 | 770 |
| 適用外 | 426 |
| 判定待ち | 0 |

Robot の `Should Load Repls` は Linux x64 のブラックリスト条件で 225 構成に展開しました。実際の `using` を追跡すると、STM32H7 の Cortex-M7 ベースを継承するのは、参照・Discovery・Nucleo の 3 ボードと CPU の 4 定義です。残る 218 行には、ケースが要求する個別 REPL と STM32H7 継承がないことを記録しました。NUnit `ShouldGuardPeripheralReads` は `cpuCount=1,2,3,4` をそれぞれ含めます。汎用 ICPU のモック台数で、実機のコア数は条件に含まれません。

固定 STM32F746G のボタン割り込みと STM32L072 ELF のシンボルケースは、必要なボード・ファームウェア・観測点を記録して適用外としました。一方、F4 の REPL を足場にした共通 Cortex-M の WFE・BASEPRI・SysTick と、Cortex-M4F の CMSIS-DSP は H7 に移植可能なため対象です。繰り返し指定とテスト本文のループは、同一構成での回数・刺激・順序として判定条件に残します。同一リポジトリ・ファイル・ケース・構成の重複だけを除くよう生成器を更新し、今回の生成結果には重複がありませんでした。

再生成には `full-upstream-network` 節の固定アーカイブ取得と `inventory-stm32h7.py` の手順を使います。`cargo test` と Shell・I/O・表示/SD・LSM6DSO の診断スモークを実行しました。固定上流との差分を残す部分移植は等価合格数に含まれません。対象ケースの実装・等価検証は残っています。

### full-h7-tim8-upstream

固定上流 `stm32h7.repl` の TIM8 登録を APB2ENR/APB2RSTR のビット 1 に接続しました。名前付き IRQ と出力・入力リストを読み込み、Break は NVIC 43、Update は 44、Trigger/Commutation は 45、CaptureCompare は 46 に接続します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
bash tests/stm32h7-tim8-smoke.sh /tmp/Shellスモークのログディレクトリ
target/release/penv run \
    --script ../renode/scripts/single-node/stm32h7_renode_reference_board.resc \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1000000
```

TIM8 スモークは上流の登録と GPIO 出力追記をそのまま診断 REPL に加え、上流参照の Zephyr ELF を起動します。固定 `STM32_Timer.cs` の `EventGeneration`、`HandleResetMode`、`UpdateInterrupts` と照合した刺激・観測点は次のとおりです。

| 刺激 | 実測した TIM8 SR と NVIC ISPR1 |
| --- | --- |
| EGR の CCxG/COMG/TG と予約ビット | SR 0、関連 IRQ 保留なし |
| DIER.UIE と EGR.UG | SR `0x1`、IRQ 44 の `0x1000` |
| TI1 入力・slave reset mode・DIER.TIE | CNT 0、SR `0x40`、IRQ 45 の `0x2000` |
| タイマーの CC1 比較一致・DIER.CC1IE | SR `0x2`、IRQ 46 の `0x4000` |

固定モデルでは Break・Commutation の出力は低レベルです。スモークでは APB1L が TIM8 を有効にしないことと、APB2 リセット後に ARR が `0xffff`、DIER が 0 に戻ることも確認します。証拠は `monitor.log`、`0.uart.log`、`boot.resc` です。今回の刺激は固定モデルのソースから導出しました。固定 Renode の直接実行や既存 Robot の等価実行は未確認なので、上流ケース台帳の合格数には加えていません。外部トリガーは `timer8 OnGPIO` を使用し、GPIO ポートからタイマーへの代替入力配線は残件です。

未変更 RESC は TIM8 を通過し、`platforms/cpus/stm32h7.repl:316` の `timer1: unsupported STM32 timer RCC alias TIM1` で停止します。上流 TIM1 は APB2 のビット 0、IRQ 24〜27 と GPIO 代替機能の追記を使用します。

### h7-tim1

TIM1 を APB2ENR/APB2RSTR のビット 0 に結合しました。TIM8 で実装した分離出力を使い、固定上流 REPL の Break → NVIC 24、Update → 25、Trigger/Commutation → 26、CaptureCompare → 27 を読み込みます。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
bash tests/stm32h7-tim8-smoke.sh /tmp/Shellスモークのログディレクトリ timer1
bash tests/stm32h7-tim8-smoke.sh /tmp/Shellスモークのログディレクトリ
```

TIM1 の登録と GPIO 出力追記を固定上流からそのまま取り込み、公開 Zephyr 参照 ELF の起動後に RCC・SR・NVIC ISPR0 を照合しました。APB1LENR の同じビットではゲートが開かず、APB2ENR で ARR が `0xffff` を返しました。Update、TI1 の slave reset trigger、CC1 比較一致はそれぞれ `0x02000000`、`0x04000000`、`0x08000000` を NVIC に保留し、リセットで初期値へ戻りました。固定モデルどおり Break と Commutation の保留は発生しません。TIM8 も同じスモークで回帰確認しました。

未変更の上流 RESC は TIM1 を通過し、`platforms/cpus/stm32h7.repl:100` の `gpioPortK: missing SYSCFG port selector` で停止します。GPIO からタイマーへ向かう代替入力のファンアウトと、ピンの AF 選択に応じた配線が残件です。台帳の `Should Load Repls` に証拠を追記し、REPL 全体のロードが未達のため `OPEN` を維持しました。

### h7-board-pass

H753ZI 固有の 25 件のうち、Ethernet echo・Blinky・Button の 3 件を部分検証し、台帳に記録しました。残り 22 件は `OPEN` です。今回の上流等価合格は 0 件です。使用した上流テストは `tests/platforms/nucleo_h753zi.robot` です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
bash tests/stm32h7-zephyr-smoke.sh
base=/tmp/Shellスモークのログディレクトリ
bash tests/stm32h7-nucleo-board-smoke.sh "$base"
bash tests/stm32h7-tim8-smoke.sh "$base" timer1
bash tests/stm32h7-tim8-smoke.sh "$base" timer8
```

- **Blinky** は上流 ELF の UART 起動行と LED 状態を確認しました。GreenLED に対応する GPIOB0 を 10 ms ごとにサンプリングし、点灯・消灯とも約 1 秒の区間を計 8 回確認しました。
- **Button** は上流 ELF の UART プロンプトを確認し、上流と同じ `UserButton1 Press` / `Release` を実行しました。UART の押下通知と GreenLED の消灯・点灯・消灯を確認しました。両ケースとも診断 REPL を使用し、Robot の LED Tester と同じ停止条件は未移植です。
- **Ethernet echo** は上流の両 ELF を診断 REPL 上で仮想スイッチに接続しました。17 仮想秒の連続接続でサーバーの応答とクライアントの照合成功カウンター `0x44` を確認しました。クライアント UART は `<err> net_ic` の途中で止まり、8 秒後に切断する対照実験では `Received and compared … all ok` が出力されました。連続接続中に UART が止まる原因と、上流が要求する 4 組の順序照合は未確認です。

GPIO の数値出力から複数のタイマー入力へ配線できるようにしました。上流の PA7 ファンアウト定義を取り込んだ診断 REPL で公開 Zephyr ELF を起動し、PA7 の立ち上がり 1 回で TIM1 と TIM8 の CNT が 0、SR が `0x40` になることを確認しました。Zephyr Shell と TIM1/TIM8 の回帰も通過しました。未変更の参照 RESC と Nucleo REPL は GPIOK を通過し、`platforms/cpus/stm32h7.repl:206` の `lpuart1: unsupported USART IRQ wiring` で停止します。USART の複数登録と EXTI 配線を次に照合する必要があります。

ログ、終了状態、診断用 RESC/REPL は `~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr` に保存しました。以下は同じ診断と未変更定義の再実行コマンドです。

```bash
e=~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr
for scenario in echo-long echo-drain; do
    target/release/penv run --script "$e/$scenario/echo.resc" \
        --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" \
        --max-steps 18000000 --uart-log-dir "$(mktemp -d)"
done
target/release/penv run --script "$e/fanout/boot.resc" \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1500000
target/release/penv run --script ../renode/scripts/single-node/stm32h7_renode_reference_board.resc \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1000000
target/release/penv run --script "$e/nucleo-load.resc" \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1000000
```

### backlog-network-e9661f3ff43b-1

複数登録を持つ上流 STM32H7 USART/LPUART を読み込み、既存の EXTI ルートに接続しました。RCC の APB1L、APB2、APB4 の有効化・リセットを適用し、UART 入力はクロックと受信が有効な最初の USART に渡します。リンクした USART3 でゲート、受信、EXTI28 → NVIC39、リセットをテストしました。上流定義では未配置の DBGMCU 領域は固定 Renode の未配置アクセスに合わせて 0 を返します。従来の診断 REPL にある DBGMCU モデルは引き続き使用できます。

上流 `tests/unit-tests/platform_descriptions.robot` の `Should Load Repls` を STM32H7 の固定パラメーター 7 件で移植しました。各 CLI プロセスで新規マシンを作成し、同じ REPL のロードの成否を観測して破棄します。Nucleo、参照ボード、CPU 定義 4 件の計 6 件が等価合格です。Disco は I²C4 アドレス `0x38` の `Input.FT5336` と GPIOK7 の割り込みが未対応です。台帳全体では対象 785 件のうち 6 件が等価合格、17 件が部分検証、762 件が未達です。

未変更の参照 RESC で公開 Zephyr が起動し、UART の `kernel version` に `Zephyr version 4.4.99` と応答しました。未変更 Nucleo REPL でも上流の Blinky、Button のスモークは通過しました。LED Tester と停止条件が未移植のため部分検証のままです。Ethernet echo は同じ構成で 10 仮想秒以内にサーバー応答 10 回と照合成功カウンター `0x2c` を観測しました。クライアント UART の比較行と順序条件は未達です。CubeMX の Ethernet リンクケースは公開 ELF の hard-float ABI をローダーが拒否しました。上流の `netif link up` は未観測です。

ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-usart.gG85D2ya` に保存しました。参照 RESC のシェルは応答後に手順中の命令上限に達するため、コード 2 で終了します。ロードスクリプトは Disco の未達によりコード 1 を返し、全構成の個別終了状態を記録します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-board-smoke.sh
input=$(mktemp)
printf 'kernel version\r' > "$input"
target/release/penv run --script ../renode/scripts/single-node/stm32h7_renode_reference_board.resc \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1800000 \
    --uart-event "1250000:$input"
e=~/.local/state/penv-dispatch/evidence/h7-usart.gG85D2ya
target/release/penv run --script "$e/echo/echo.resc" --platform-root ../renode \
    --asset-dir "$PENV_ASSET_DIR" --max-steps 11000000 --uart-log-dir "$(mktemp -d)"
target/release/penv run --platform ../renode/platforms/boards/nucleo_h753zi.repl \
    --platform-root ../renode --max-steps 3000000 "$PENV_ASSET_DIR"/cubemx--stm32h7-eth-test.elf-s_1231644-3e808bfc20a3a96ad304acbc94c7dbd6b06245e5
```

### backlog-network-e51dff8e297b-1

STM32H747 Disco の未変更 REPL が登録する FT5336 を実装しました。I²C4 の `0x38` に接続し、ボード指定の回転と Y 軸反転、接触・解放・座標の報告、ベンダー ID、GPIOK7 の割り込みパルスを扱います。入力にはモニターの `sysbus.i2c4.touchscreen MoveTo 50 60`、`Press`、`Release` を使います。

- `Should Load Repls` の STM32H7 対象 7 構成が合格しました。以前失敗していた Disco を含みます。
- 固定 Infrastructure の `FT5336Test.cs` 4 件を同じ回転・反転、最大座標、移動・押下、読み出し長、状態・座標の期待値で移植しました。4 件を台帳の分母に追加しました。対象 789 件のうち等価合格 11 件、部分検証 17 件、未達 761 件です。
- Disco の未変更 REPL で公開 STM32H747 表示 ELF を実行し、モニターで I²C4 の送受信レジスターを操作しました。接触座標 `(420, 50)`、解放状態、ベンダー ID `0x51`、EXTI7 と NVIC23 の保留を確認しました。この試行の I²C 操作はモニターから行っています。表示 ELF の単独実行では最初の 1 仮想秒に UART 出力が得られず、ゲストドライバーからのタッチ処理と表示連携は未確認です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-board-smoke.sh
target/release/penv run --script tests/stm32h7-ft5336.resc \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1500000
```

モニター出力の末尾列は順に `0x00000080 0x00800000 0x00000081 0x000000a4 0x00000000 0x00000032 0x00000000 0x00000041 0x00000051` です。検証ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-ft5336.AQ9WNxRa` に保存しました。

### backlog-network-5f23f917ea51-1

`nucleo_h753zi.robot` の `Should Blink Led` と `Should See Button Press` を移植しました。両方とも未変更 Nucleo REPL と固定上流の公開 ELF を使います。モニターは名前で選んだ USART3 の出力を待ち、上流と同じ順序で LED の観測とボタン操作へ進みます。

- Blinky は起動バナーの途中一致と LED 状態行の完了を待ち、GreenLED の変化を 8 仮想秒追跡します。最初のエッジで同期し、その後の点灯・消灯区間を各 1 秒 ±5% で照合します。今回は 6 完全区間を確認しました。
- Button は起動と操作案内の後に GreenLED の消灯を確認し、上流と同じ UserButton1 を押下します。UART の押下通知を待って点灯を照合し、解放後に消灯を照合します。LED の各期待値の期限は 1 仮想秒、UART の各待機期限は 8 仮想秒です。

CLI の UART 待機はこの移植で使うリテラル部分一致と複数候補に対応します。Blinky の上流正規表現は等価な 2 候補で表現しました。Ethernet echo のような数値付き正規表現とクライアント出力の不足は残件です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-led.Fm3d6yZW` に保存しました。点灯期待値の変更とボタン押下の削除で、期間違反と UART タイムアウトも確認しました。全 789 件のうち等価合格 13 件、部分検証 15 件、未達 761 件です。

### backlog-network-d636d1885df8-1

未変更 Nucleo REPL と上流の公開 ELF で、ADC DMA と Zephyr Encryption Sample の 2 ケースが等価合格しました。ADC DMA は USART3 で上流と同じ 7 件の `PASS` 行を順番に照合しました。暗号サンプルは従来の ECB の次に CBC で停止していたため、STM32H7 CRYPTO に CBC の IV 更新と CTR の 32 ビットカウンターを実装しました。ECB・CBC・CTR の暗号化と復号の `Match` 行が上流と同じ順序で出ました。

長文 UART ケース向けに、名前付き USART への行入力、禁止文字列の監視、REPL 周波数と UART 設定に基づく受信間隔を実装しました。上流と同じコマンドと CR を入力し、50 ms の実行後にプロンプトを照合しました。さらに 0.7 仮想秒の対照観測を追加すると `RX ring buffer full` が出るため、部分検証に留めています。モニターが禁止文字列を検出して失敗することも、この対照で確認しました。

CAN ループバックは初期化後の `Counter received: 0` が 8 仮想秒以内に出ず、FDCAN の送信・受信 FIFO ループバックが未実装です。割り込みケースは `test_arm_esf_collection` 中に `0x0800c460` の Thumb `0xde5a` で停止します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
logs=$(mktemp -d)
bash tests/stm32h7-nucleo-uart-line.sh "$logs"
printf 'emulation RunFor "0.7"\n' >> "$logs/case.resc"
target/release/penv run --script "$logs/case.resc" --platform-root ../renode \
    --asset-dir "$PENV_ASSET_DIR" --max-steps 18000000 --uart-log-dir "$(mktemp -d)"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-uart-line.m4RnjA3M` にあります。ADC DMA・暗号サンプルは `ztest/`、長文の対照は `paced-after/` です。台帳は対象 789 件のうち等価合格 15 件、部分検証 16 件、未達 758 件です。

### backlog-network-160de3171208-1

Cortex-M の 16 ビット `UDF` を同期例外として処理し、命令アドレスを例外フレームに保存します。UsageFault が有効で優先度が許せば UsageFault に入り、それ以外では HardFault に昇格します。CFSR.UNDEFINSTR と HFSR.FORCED の記録とクリア、およびハンドラー内の SVC の優先度判定を実装しました。

未変更の Nucleo REPL と `nucleo_h753zi.robot` の公開割り込み ELF で `test_arm_esf_collection` が合格しました。後続 `test_arm_interrupt` は期待した reason 2 が処理されず失敗し、`test_arm_user_interrupt` は PC `0x08001c0c` から Thumb 状態を選ばないアドレス 0 に分岐して停止します。UART と停止理由を記録し、Robot ケース全体は部分検証としました。

共通 SysTick モデルを上流の即時リロードと周期に合わせ、`arm-cortex-m-interrupts.robot` のリロード設定順が異なる 2 ケースを移植しました。各テストは上流と同じ 100 Hz、RELOAD 10、同じ CSR・RVR・CVR 書き込み順を使い、0/90/100/110 ms 時点の CVR が 10/1/10/9 になることを確認します。汎用タイマーの検証なので F4 のボード固有機器への刺激はありません。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test upstream_count_down_with_reload
cargo build --release
e=~/.local/state/penv-dispatch/evidence/h7-usagefault.F5wFPNUj
target/release/penv run --script "$e/interrupt.resc" \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --uart-log-dir "$(mktemp -d)"
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

ログと終了状態は上記の証拠ディレクトリに保存しています。割り込み ELF の最終ログは `final/` にあります。台帳は対象 789 件のうち等価合格 17 件、部分検証 17 件、未達 755 件です。

### backlog-network-53ae79821516-1

共通 Cortex-M/NVIC モデルに WFE の待機と SEV のイベントを実装しました。SCR.SEVONPEND が有効な場合、無効化されている IRQ も保留への遷移でイベントを発生させます。WFE はイベントで再開し、IRQ の許可と優先度に基づく例外処理は従来どおりです。BASEPRI・PRIMASK・FAULTMASK から求める優先度の下限判定を共通化しました。

上流 `arm-cortex-m-interrupts.robot` の残り 2 件を Rust テストに移植しました。WFE ケースは同じ `WFE; STR; B`、SCR=0x10、IRQ5 で、刺激前の対象メモリーが 0、刺激後 1 仮想秒以内に `0xDEADBEEF` になることを照合します。BASEPRI ケースは同じ `0x50` と優先度の比較を行い、リセット前の一致とリセット後の不一致を確認します。両ケースとも F4 固有の周辺機器を使わないため、H7 でも使用する共通モデルで検証しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test upstream_wfe` と `cargo test upstream_basepri` で個別に再現できます。ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-wfe.Vp3mSKus` に保存しました。台帳は対象 789 件のうち等価合格 19 件、部分検証 17 件、未達 753 件です。Nucleo の公開割り込み ELF の後続サブテストには、前記の例外理由とユーザーモード分岐の未達が残っています。

### backlog-network-1d4470862620-1

上流 `tests/unit-tests/arm-ge-flag.robot` の固定 Cortex-M7 ケース 6 件を移植しました。Thumb-2 の符号付き・符号なし並列加減算で、各 8/16 ビット演算の結果と GE ビットを更新し、`SEL` が GE に従ってバイトを選択します。APSR の GE を読み出し・書き込みと例外フレームに反映します。命令実装とテストは担当ローカルの nRF52 作業ツリーにある共通部分を参照しました。

各テストは上流と同じ 10 命令を実行し、R4 を照合します。固定上流のアセンブリを Cortex-M7 向けに再アセンブルし、6 件それぞれの 40 バイトと R4 の期待値が移植版と一致することを確認しました。タイトルが `usub8` のケースでは、固定上流本文どおり `ssub8` を実行しています。このケースが符号なしバイト減算のカバレッジを与えるわけではありません。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test thumb::ge_upstream
cargo build --release
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

未変更 Nucleo REPL と公開 Zephyr ELF で ADC DMA、暗号サンプル、Blinky、Button の回帰が通過しました。アセンブリ照合、実行ログ、終了状態は `~/.local/state/penv-dispatch/evidence/h7-ge.aGNitIHh` にあります。台帳は対象 789 件のうち等価合格 25 件、部分検証 17 件、未達 747 件です。

### backlog-network-1d617f3607ae-1

上流 `VEML7700.robot` のポーリング 4 ケースを移植しました。Nucleo の未変更上流 REPL に、Robot と同じ `als: Sensors.VEML7700 @ i2c1 0x10` を追加します。モニターからの名前付き照度変更を実装し、公開 Zephyr 4.2 と 4.4 ELF で同じ設定値と USART3 の出力を比較しました。4.2 は `0 → 499 → 29 → 10000`、4.4 は `0 → 499` です。

固定上流の CSV を上流の `csv2resd.py` で定周期・タイムスタンプ付き RESD に変換し、それぞれを同じ公開 Zephyr 4.2 ELF に供給しました。センサーは仮想時刻に対応するサンプルを読み、終了後は最後の値を保持します。両ケースで `0 → 499 → 12000 → 12000` を確認しました。実装対象は RESD v1 の照度チャンネル 0 の単一ブロックです。

上流 VEML7700 Shell ELF は USART3 のプロンプトまで起動しました。500 lx を設定して上流コマンドを送った後、8 仮想秒以内にセンサー応答を観測できず、このケースは部分検証のままです。参照ボードの Zephyr Shell と上流 REPL ロード 7 構成は回帰確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
venv=$(mktemp -d)
python3 -m venv "$venv"
"$venv/bin/pip" install -r ../renode/tools/csv2resd/requirements.txt
PYTHON="$venv/bin/python" bash tests/stm32h7-veml7700-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-veml.QF6dq3bd
target/release/penv run --script "$e/shell-wait/case.resc" \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 17000000
```

RESD、UART、終了状態は `~/.local/state/penv-dispatch/evidence/h7-veml.QF6dq3bd` に保存しました。台帳は対象 789 件のうち等価合格 29 件、部分検証 17 件、未達 743 件です。

### backlog-network-a8bb256e028f-1

上流 `LPS25HB.robot` の 4 ケースを移植しました。公開 Zephyr 4.2/4.4 ELF を未変更の Nucleo REPL と上流と同じ `i2c1 0x5D` のセンサー登録で実行しました。実行時の温度・気圧変更、範囲外の入力で前の値が維持される動作、各段階の USART3 出力を上流順に照合しました。圧力プロパティの設定と読み戻しは `1023.346923828125`、直接レジスター読み出しは `8D F5 3F` を確認しました。

共有 RESD v1 デコーダーでチャンネル 0 の温度・気圧を読み、固定モデルと同じセンサー出力データレートでサンプルの反映時刻を求めます。上流変換器が生成した 25 Hz・開始 70 ms の RESD で、3 組のサンプルと終端後の既定値への復帰 2 回を確認しました。RESD はサンプル種別ごとに単一ブロックを扱い、LPS25HB の終端復帰は定周期ブロックで検証しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test upstream_pressure_property_and_direct_read
cargo build --release
venv=$(mktemp -d)
python3 -m venv "$venv"
"$venv/bin/pip" install -r ../renode/tools/csv2resd/requirements.txt
PYTHON="$venv/bin/python" bash tests/stm32h7-lps25hb-robot.sh "$(mktemp -d)"
PYTHON="$venv/bin/python" bash tests/stm32h7-veml7700-robot.sh "$(mktemp -d)"
```

検証ログ、RESD、終了状態は `~/.local/state/penv-dispatch/evidence/h7-lps-live.w4haoFY7` にあります。LPS25HB の最終ログは `paced/` にあり、VEML7700 ポーリング 4 件と参照 Zephyr Shell の回帰も通過しました。台帳は対象 789 件のうち等価合格 33 件、部分検証 14 件、未達 742 件です。

### backlog-network-961099fa88c2-1

LSM6DSO に加速度・角速度の独立した RESD ストリームを追加しました。固定上流の CSV と変換器で、チャンネル 0、12 Hz、開始 100 ms の RESD を生成します。ゲストが設定した出力データレートでサンプルを更新し、ストリーム終了後は設定された既定値へ戻ります。従来の CSV 診断入力も引き続き使えます。

未変更 Nucleo REPL に Robot と同じ I²C1 の `0x6A` と PC13 の割り込み配線を追加し、固定上流の ELF で USART3 の順序と 2 仮想秒の各期限を照合しました。基本ポーリング、基本割り込み、割り込み RESD、Errata 対応ドライバーの 4 件が合格しました。

ポーリング RESD は部分検証です。第 3 組の加速度は 3,149,860 ラウンドで一致します。次の角速度には第 3 組の期待値が出ず、ゲストは既定値を出力します。固定 RESD は約 3.1 仮想秒で終了するため、このポーリングでの出力サンプルの保持とゲスト読み出しの時系列が残件です。移植スクリプトは全 5 件を実行し、この未達をコード 1 で報告します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
venv=$(mktemp -d)
python3 -m venv "$venv"
"$venv/bin/pip" install -r ../renode/tools/csv2resd/requirements.txt
PYTHON="$venv/bin/python" bash tests/stm32h7-lsm6dso-robot.sh "$(mktemp -d)"
PYTHON="$venv/bin/python" bash tests/stm32h7-lps25hb-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
```

検証ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-lsm-resd.YFHMg3EA` にあります。LSM6DSO の最終実行は `final/` です。台帳は対象 789 件のうち等価合格 37 件、部分検証 10 件、未達 742 件です。

### backlog-network-89ddefe9bc0e-1

IIS2MDC の RESD 入力を実装し、固定上流のポーリングとトリガーの 2 ケースが等価合格しました。上流コンバーターは開始 3 秒、2 Hz の 9 ブロックを生成します。共有デコーダーは隣接する定周期ブロックを連結できます。未変更の STM32H753 CPU REPL に上流と同じ I²C1 と GPIOG11 を配線し、同じ ELF で USART2 の全 9 組と各 8 秒の期限を照合しました。

INA228 の I²C 設定・測定レジスター、ADC 周期、電力量と電荷の積算、電圧 2 チャンネルと温度の RESD 入力を実装しました。定周期・タイムスタンプ付きの両ケースで、同じ上流 ELF・RESD・3 秒の行待機期限を使いました。電圧・電流・電力・温度は全行で許容差内でした。電力量と電荷が許容差外のため、両ケースは部分検証です。

INA228 の最初の非ゼロ行は約 2.186 仮想秒で出力され、電力量 330.813184 J、電荷 6.896582 C でした。固定上流の期待値は 288.29 J と 6.01 C です。積算区間を左右する ADC 更新・ゲスト読み出し・リセットの時系列の照合が残っています。許容差を保持しており、INA228 のスクリプトはコード 1 を返します。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
venv=$(mktemp -d)
python3 -m venv "$venv"
"$venv/bin/pip" install -r ../renode/tools/csv2resd/requirements.txt
PYTHON="$venv/bin/python" bash tests/stm32h7-iis2mdc-robot.sh "$(mktemp -d)"
PYTHON="$venv/bin/python" bash tests/stm32h7-ina228-robot.sh "$(mktemp -d)"
PYTHON="$venv/bin/python" bash tests/stm32h7-lps25hb-robot.sh "$(mktemp -d)"
```

ログ、RESD、終了状態は `~/.local/state/penv-dispatch/evidence/h7-ina228.lLR6tsyx` にあります。INA228 の比較結果は `ina-final/*/compare.log` に保存しました。LPS25HB と上流 REPL 7 構成の回帰も通過しました。台帳は対象 789 件のうち等価合格 39 件、部分検証 10 件、未達 740 件です。

### backlog-network-fa475fcf2125-1

STM32H7 RCC の発振器・PLL の書き込み制約とリセット値を修正しました。選択中のシステムクロック、動作中の PLL のクロックソース、HSE CSS の保持、PLL1 の無効な DIVP1 値を固定上流の NUnit 7 件と照合しました。公開 Zephyr ELF が使う `SMLAxy` と APSR.Q、内部クロック校正レジスターも実装しました。

未変更 STM32H743 REPL と上流 ELF で、HSE 8、HSE 8 CSS、HSI 64、PLL CSI 96、PLL HSE 96、PLL HSI 96 の 6 Robot ケースが等価合格しました。CSI 4 は USART3 が無出力のまま 8 仮想秒でタイムアウトします。短い追試の命令上限時点では PC `0x0800479c` を観測しましたが、停止原因は未確定です。RCC の TIM1 リセットログ、周波数照会、ゲート状態照会を使う残り 3 Robot ケースも未達です。

LTDC ではボタン登録にピン番号を省き、接続先のピンだけを書く固定 REPL の構文に対応しました。その構成の公開表示 ELF は、上流 STM32H743 REPL に未配置の DSI 領域 `0x50000430` をアクセスして停止します。シャドーレジスターの両ケースは既存の診断検証に基づく部分検証のままです。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test upstream_rcc_tests
cargo build --release
bash tests/stm32h7-rcc-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-ltdc-robot.GpG8dofF
target/release/penv run --script "$e/start.resc" --platform-root ../renode \
    --asset-dir "$PENV_ASSET_DIR" --max-steps 45000000
```

RCC スクリプトは CSI 4 の未達によりコード 1 を返します。ログと終了状態は上記の証拠ディレクトリにあり、RCC の最終実行は `rcc-rules/` です。参照 Zephyr Shell、ADC DMA、暗号サンプル、Blinky、Button、REPL ロード 7 構成は回帰を通過しました。台帳は対象 789 件のうち等価合格 52 件、部分検証 10 件、未達 727 件です。

### backlog-network-3ea9295d2060-1

未変更 STM32H743 REPL で、ファームウェアを実行しない `STM32H7_RCC.robot` の TIM1 3 ケースを移植しました。APB2RSTR への `0x1` で実リセットを実行し、`Resetting TIM1` を記録しました。リセット前後の周波数はどちらも 64 MHz でした。APB2ENR へ上流と同じ値を書いて、有効・無効・有効の順に確認しました。

登録されたタイマーのゲートは上流モデルと同じく初期状態で有効です。タイマー周波数は RCC のシステムクロックとドメイン・APB 分周から求めます。別のプローブで HSI 64 MHz から HSE 8 MHz に切り替え、HPRE・D2PPRE2・TIMPRE による `4 → 2 → 4` MHz も確認しました。既存 TIM1/TIM8 実 ELF スモークのクロック停止確認には、APB2ENR を明示的にクリアする操作を追加しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-rcc-timer-robot.sh "$(mktemp -d)"
bash tests/stm32h7-rcc-robot.sh "$(mktemp -d)"
base=~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr/shell
bash tests/stm32h7-tim8-smoke.sh "$base" timer1
bash tests/stm32h7-tim8-smoke.sh "$base" timer8
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-rcc-timer.0zFmYqdC` に保存しました。クロック構成の公開 ELF は既合格の 6 件が再度通過しました。CSI 4 は引き続き 8 仮想秒以内に USART3 出力がありません。台帳は対象 789 件のうち等価合格 55 件、部分検証 10 件、未達 724 件です。

### backlog-network-6caf4b37ab9c-1

STM32 タイマーの出力比較で、対応する CCxIE が有効な場合に CCxIF を立てるようにしました。固定 Renode の STM32_Timer と同じ条件です。比較割り込みをマスクしたまま PWM 出力は動作します。

上流 `ComparingTimerTests.cs` から 4 件を移植しました。1 MHz、比較値、経過時間、CNT への書き戻しを揃え、CNT・CCR1 と各区間の比較イベント数を照合しました。汎用タイマーが周期を上限とするフィクスチャに対し、STM32 の ARR にはラップ直前の値である周期 − 1 を設定しています。汎用 `Compare` setter の範囲外例外は、STM32 の CCR 書き込みで観測する契約が違うため未達のままです。

公開 Zephyr 参照 ELF を実行し、TIM3 の PWM 出力を有効にした状態で CC1IE の前後を観測しました。ステータスはマスク中が `0x1`、有効化後が `0x3` で、比較フラグだけが追加されました。TIM1/TIM8 実 ELF スモークと RCC の TIM1 3 ケースも通過しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test comparing_timer_cases
cargo build --release
e=~/.local/state/penv-dispatch/evidence/h7-compare.z8PftPwj
target/release/penv run --script "$e/tim3-final/boot.resc" \
    --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 1500000
bash tests/stm32h7-rcc-timer-robot.sh "$(mktemp -d)"
```

ログと終了状態は上記の証拠ディレクトリに保存しました。台帳は対象 789 件のうち等価合格 59 件、部分検証 10 件、未達 720 件です。

### backlog-network-d77017f82834-1

`penv check-platform --from-string REPL` を追加し、ファイル入力と同じ REPL パーサーと構築処理を使うようにしました。上流 `tests/unit-tests/platform_descriptions.robot` のインライン 3 ケースを移植しました。空文字列は正常終了し、存在しない `using "invalid"` と抽象 `CPU.TranslationCPU` は上流の期待診断を含むエラーで終了します。上流ではモニターコマンドで渡す 3 つの入力を、移植版は CLI のインライン検証に渡します。既存マシンにインライン定義を追加するモニター操作は残件です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

インラインから有効な STM32H743 REPL を `using` する経路、固定上流 REPL のロード 7 構成、公開 Nucleo Zephyr ELF の Blinky と Button も通過しました。ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-repl-errors.6QAE3tnn` に保存しました。台帳は対象 789 件のうち等価合格 62 件、部分検証 10 件、未達 717 件です。

### backlog-network-503a5ccb5864-1

共有 `UsingTests.cs` の 8 件を移植しました。同じ REPL 内の重複 `using`、直接・間接循環、存在しないファイルを、上流と同じインクルード構成と診断条件で確認しました。Unix では絶対パス、同一・親ディレクトリへの相対パス、バックスラッシュを含むファイル名を、上流と同じ入力と解決結果で照合しました。

Windows パス 3 件は固定上流が非 Windows ホストで `Assert.Ignore` とするため、台帳を対象外に訂正しました。参照と `local` スコープ、合成グラフの属性上書き順を扱う残り 9 件は引き続き対象です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test platform::using_cases
cargo build --release
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

固定 REPL の 7 構成、インライン REPL の 3 ケース、公開 Nucleo Zephyr ELF の Blinky と Button は回帰を通過しました。ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-using.iKgL0z6U` に保存しました。台帳は対象 786 件のうち等価合格 70 件、部分検証 10 件、未達 706 件です。

### backlog-network-51d0b6ddb4c3-1

REPL の `using` グラフを子から親へ合成し、同じ階層では宣言順に属性を適用します。型付き宣言が後に現れる場合も、先行する属性の追記先を解決します。型付き宣言の重複と、同じ宣言内で重複したプロパティは既存の診断を維持します。

固定上流 `UsingTests.cs` の属性上書き順 5 件を移植しました。固定 REPL 文字列を使い、上流の MockCPU が保持する文字列 `Placeholder` の期待値を合成結果で照合します。実 STM32H743 でも、`using` より前に置いた GPIO の属性が構築後の MMIO に反映されることを確認しました。MockCPU の参照解決と `local` スコープを扱う 4 件は残っています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test should_follow_attribute_override_order
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-using-order.bX4dvaHY
target/release/penv run --script "$e/gpio.resc" --platform-root ../renode
```

検証ログと終了状態は上記のディレクトリに保存しました。台帳は対象 786 件のうち等価合格 75 件、部分検証 10 件、未達 701 件です。

### backlog-network-35d1690c36de-1

REPL にブロックコメント処理を追加し、引用符の内側にあるコメント記号を保持しました。固定上流 `PreLexerTests.cs` の 3 件を移植し、閉じていない文字列、インデント構文で複数行コメントの終了直後にコードが続く入力、行頭のブロックコメント直後にコードが続く入力を、同じ入力で構文エラーとして検出しました。実 STM32H743 REPL の GPIO 追記でも、コメントを含む入力が構築され、設定値 `0x55555555` が MMIO に反映されることを確認しました。

`UsingTests.cs` の参照 4 件にはファイル単位の `local` スコープと型付き前方・後方参照が必要です。これらは未達のままです。ブロックコメントの処理と、任意の複数行インライン属性ブロックの構文解析は別の段階であり、後者の対応も残っています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo test platform::comment_cases
cargo build --release
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-repl-comments.Zk9Rz9Cz
target/release/penv run --script "$e/probe.resc" --platform-root ../renode
```

ログと終了状態は上記のディレクトリに保存しました。台帳は対象 786 件のうち等価合格 78 件、部分検証 10 件、未達 698 件です。

### backlog-network-58f9301a8fe9-1

REPL の波括弧内で複数行の属性を読み込み、属性間のセミコロンと任意の末尾セミコロンを処理します。固定上流 `ParserTests.cs` の 9 件を移植し、同じ入力で 4 件の解析結果と 5 件の構文エラーを照合しました。STM32H743 では、複数行コメントを含む GPIO 属性ブロックを組み立て、MMIO で設定値 `0x55555555` を確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-inline-attrs.hEQbdrTE
target/release/penv run --script "$e/gpio.resc" --platform-root ../renode
```

上記の証拠ディレクトリに実行ログと終了状態を保存しました。固定 STM32H7 REPL 7 構成、構文診断 3 件、公開 Zephyr ELF の Blinky と Button が通過しました。台帳は対象 786 件のうち等価合格 87 件、部分検証 10 件、未達 689 件です。型付き参照と `local` スコープ、登録ブロックを含む他の構文ケースは引き続き未達です。

### backlog-network-7bccc7d5790b-1

REPL の波括弧付き登録リストを、周辺機器の型に依存せず構文解析します。登録の順序を保持し、登録間のセミコロンと任意の末尾セミコロンを処理します。登録先が空か、上流で識別子に使えない文字で始まる場合は構文エラーにします。登録値の文字列や入れ子のオブジェクトに含まれるセミコロンは登録の区切りにしません。固定上流 `ParserTests.cs` の8件を同じ入力で実行し、4件の受理と4件の拒否を照合しました。2登録のケースでは名前、型なし宣言、登録先、アドレス、順序も照合しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` は119件が通過し、STM32H7 と LiteX の双方向フレーム交換も含みます。固定 STM32H7 REPL 7構成、構文診断3件、公開 Nucleo Zephyr の Blinky・Button ELF も通過しました。ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-repl-registration.iw73Tr6H` にあります。

台帳は対象786件のうち等価合格95件、部分検証10件、未達681件です。登録値の完全な型別構文検証、単一登録の範囲・文字列の専用ケース、型付き参照と `local` スコープのケースは残っています。

### backlog-network-aa7e424888f6-1

単一登録と登録リストで登録先・登録値の解析を共有しました。固定上流 `ParserTests.cs` の3件を同じ入力で実行し、宣言名、型なし宣言、登録先、登録値なし・範囲・文字列の各期待値を照合しました。メモリーのマッピングも同じ範囲解析を使い、従来の `+サイズ` と絶対終端の両方を受け付けます。STM32H743 に絶対終端で追加した 4 KiB の RAM では、末尾ワード `0x50000ffc` に書いた `0x12345678` を読み戻しました。最初の試行は既存の SRAM バンクと重複したため、空き領域に移して再検証しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-single-registration.PjfmFq83
target/release/penv run --script "$e/range.resc" --platform-root ../renode
```

上記ディレクトリにログと終了状態を保存しました。`cargo test` 120件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Nucleo Zephyr Blinky・Button ELF が通過しました。台帳は対象786件のうち等価合格98件、部分検証10件、未達678件です。その他の登録値の型別検証、型付き参照、`local` スコープは残っています。

### backlog-network-2b82dcec1e69-1

字下げ構文の `init:` では、見出しより深い行をコマンドとして集め、同階層に戻った後の属性を通常どおり処理します。コマンドの順序と引用符内のセミコロンを保持します。固定上流 `ParserTests.cs` の3件を同じ入力と期待コマンド列で移植しました。STM32H743 では、`size` を `init:` の後に置いた RAM を構築し、初期化した2ワードを読み戻しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-init-indent.OQ0aCyhl
target/release/penv run --script "$e/init.resc" --platform-root ../renode
```

ログと終了状態は上記に保存しました。`cargo test` 121件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。台帳は対象786件のうち等価合格101件、部分検証10件、未達675件です。波括弧内の init、`init add`、汎用モニターコマンドの実行は今回の検証範囲外です。

### backlog-network-39a4f746e4d8-1

REPL の `using` で、引用符付きパスのエスケープと末尾のセミコロン列を解析します。固定上流 `ParserTests.cs` の5件を同じ入力と期待パス列で移植しました。STM32H743 では、バックスラッシュを名前に含む追記ファイルを読み込み、GPIO の設定値 `0x55555555` を MMIO から読み戻しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-using-syntax.FiABTsvM
target/release/penv run --script "$e/include.resc" --platform-root ../renode
```

上記にログと終了状態を保存しました。`cargo test` 122件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。対象786件のうち等価合格106件、部分検証10件、未達670件です。宣言と混在する `using` の専用ケース、一般プロパティ文字列のエスケープ、型付き参照と `local` スコープの検証は残っています。

### backlog-network-13672a4487f3-1

REPL の引用符付き文字列のデコーダーを `using`、登録値、IIS2MDC の `SampleCsv` で共有しました。固定上流 `ParserTests.cs` の2件を同じ入力で移植し、属性内のエスケープ済み引用符・連続バックスラッシュの復号値と、引用符付き値に続く属性の個数を照合しました。実 STM32H753 REPL でバックスラッシュを含む名前の CSV をセンサーに読み込ませました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-repl-quoted-values.VK7sb1IJ
target/release/penv check-platform "$e/sensor.repl" --platform-root ../renode
venv=$(mktemp -d)
python3 -m venv "$venv"
"$venv/bin/pip" install -r ../renode/tools/csv2resd/requirements.txt
PYTHON="$venv/bin/python" bash tests/stm32h7-iis2mdc-robot.sh "$(mktemp -d)"
```

対象786件のうち等価合格108件、部分検証10件、未達668件です。その他のモデル固有プロパティへの文字列デコーダーの適用、複数行文字列、型付き参照は残っています。

`cargo test` 123件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button と IIS2MDC polling・trigger ELF が通過しました。ログと終了状態は上記の証拠ディレクトリに保存しました。センサーの初回実行はシステム Python に `construct` がなく停止し、依存導入済みの仮想環境で再実行しました。

### backlog-network-90e05f242bfb-1

REPL のインライン属性名に固定上流と同じ先頭文字の制約を適用しました。固定 `ParserTests.cs` の6件を同じ入力で実行し、空ブロックと属性・IRQ 混在ブロックの受理、空文または裸の識別子を含む4入力の拒否を照合しました。STM32H743 では空の GPIO 追記を含む構成をロードし、範囲登録とインライン属性で追加した RAM の読み書きを確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-inline-members.IMCTbep4
target/release/penv run --script "$e/inline.resc" --platform-root ../renode
```

ログと終了状態は上記に保存しました。`cargo test` 124件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。台帳は対象786件のうち等価合格114件、部分検証10件、未達662件です。IRQ 専用の構文ケースやオブジェクト値・辞書値を含むケースは残っています。

### backlog-network-6bc447f3a884-1

REPL の IRQ リストで範囲と単一端点の展開を共通化し、STM32H7 EXTI の配線で宛先の解析を使用しました。固定上流 `ShouldParseEntryWithMultiIrqEntries` では、混在する番号・名前の送信元と宛先の番号順を照合しました。複数宛先のケースでも全宛先と展開順を照合しました。個別の範囲グループ境界を観測する上流のアサーションは残っているため、台帳では部分検証としています。EXTI モデルで複数宛先に同時配信する動作も残っています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 125件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-irq-attributes.9rE3Kilw` にあります。台帳は対象786件のうち等価合格115件、部分検証11件、未達660件です。

### backlog-network-80d50622371d-1

REPL の IRQ 解析で、範囲と単一端点のグループを宛先ごとに保持します。固定上流 `ShouldParseEntryWithMultiplexedMultiIrqEntries` と同じ入力で送信元の展開順、3宛先の順序、各宛先の端点とグループ境界を照合し、部分検証から等価合格に更新しました。STM32H7 の既存 IRQ 配線はグループを展開した番号列を使います。EXTI の複数宛先への同時配信は残っています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 125件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-irq-groups.3Clg0yOD` に保存しました。台帳は対象786件のうち等価合格116件、部分検証10件、未達660件です。

### backlog-network-ea160469cec9-1

REPL の IRQ 宛先で `名前#番号` を分解する処理を STM32H7 GPIO の SYSCFG 配線と共有しました。固定上流 `ParserTests.cs` の3件を同じ入力で移植し、番号付き・名前付き・既定の送信元、複数宛先の順序、ローカル受信先番号と入力番号を照合しました。これらは構文ケースです。実行時の複数宛先への配信は残っています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-simple-irqs.RiE7SzEV` に保存しました。台帳は対象786件のうち等価合格119件、部分検証10件、未達657件です。

### backlog-network-d851b265aa4a-1

REPL の真偽値、参照、列挙値を分類し、STM32H7 の LED とボタンの `invert` に適用しました。真偽値として認識する綴りは `true`、`false`、`True`、`False` です。固定 `ParserTests.cs` の3件を同じ入力で移植し、真偽値と参照の区別、型名付きと省略形の列挙値を照合しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-boolean-values.zehmtRdP
target/release/penv check-platform "$e/bool.repl" --platform-root ../renode
```

`cargo test` 128件、STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF、タイトルケースの真偽値を使う Nucleo 追記の構築が通過しました。ケース別結果は上記の `cases.tsv` に記録しました。対象786件のうち等価合格122件、部分検証10件、未達654件です。型付き参照の解決と、他の周辺機器への真偽値処理の適用は残っています。

### backlog-network-a41f984d1aaa-1

REPL の宣言パーサーで `local` 修飾子を保持します。空入力、単一宣言、複数宣言、同名の短縮宣言、ローカルとグローバルの区別、LF と CRLF の混在を扱う固定上流6件を一括実行しました。実行時のファイル単位の名前解決は未実装です。構築時にローカル宣言を検出すると専用診断を返すことを STM32H743 の追記で確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-declarations.nWqeeFrI
target/release/penv check-platform "$e/local.repl" --platform-root ../renode
```

`cargo test` 134件、固定 STM32H7 REPL 7構成、構文診断3件、Zephyr Blinky・Button ELF が通過しました。ケース別の結果は上記の `cases.tsv` に保存しました。台帳は対象786件のうち等価合格128件、部分検証10件、未達648件です。実行時のローカルスコープは、ファイル別の名前空間と、型付き参照および追記先の解決が必要です。

### backlog-network-02c4dcdab82b-1

REPL の字下げ属性で、スカラーのキーと値からなる辞書を解析します。固定上流の7ケースを個別の Rust テストとして移植し、同じバイナリで実行しました。空辞書、項目順、引用符付きキーと数値トークン、末尾セミコロンを照合しました。閉じ波括弧、区切り、コロンの誤りは上流が照合する診断文字列で確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 141件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-repl-dictionaries.5HgKaeRf` に保存しました。対象786件のうち等価合格135件、部分検証10件、未達641件です。入れ子の辞書・リスト・オブジェクトを辞書値に使う構文、波括弧付き属性ブロック内の辞書、辞書のモデルへの受け渡しは残っています。

### backlog-network-ffda2a9e813b-1

REPL のリスト値を解析し、STM32H7 GPIO の `invertedAFPins` に共通パーサーを適用しました。固定上流 `ParserTests.cs` の4件を個別のテストとして同じバイナリで実行し、空リスト、末尾カンマ、入れ子、数値・文字列・真偽値・参照・範囲の混在を照合しました。リスト内のインラインオブジェクトは、オブジェクトの属性ブロックを解析する処理が必要なため未達です。リストの入れ子は64レベルまでです。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 145件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果、ログ、終了状態は `~/.local/state/penv-dispatch/evidence/h7-repl-lists.pQCV9IG8` にあります。対象786件のうち等価合格139件、部分検証10件、未達637件です。

### backlog-network-66e060011c57-1

REPL の字下げ付きオブジェクト値とリスト内のインラインオブジェクトを解析します。波括弧内の属性処理は STM32H7 Ethernet の領域登録と共有しました。固定上流 `ShouldParseObjectValue` と `ShouldParseListWithInlineObject` を同じバイナリで実行し、オブジェクト型、項目数、属性名、数値トークンを照合しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 147件に STM32H7 と LiteX の双方向 Ethernet 回帰を含みます。固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF も通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-repl-objects.NqbMFJGf` に保存しました。対象786件のうち等価合格141件、部分検証10件、未達635件です。入れ子のオブジェクト属性、字下げ属性間の空行、一般の型の実行時インスタンス化は残っています。

### backlog-network-964c2dda4c3b-1

REPL のリストと辞書でスカラー値の解析を共有し、字下げ属性に書いたインラインオブジェクトの構文を検証します。固定上流 `ParserTests.cs` の残り3件を同じバイナリで実行し、型が混在する属性、登録・IRQ・init を含む複合例、`using` と宣言の組み合わせを照合しました。固定 `ParserTests.cs` の台帳66件が等価合格になりました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 150件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-parser-remaining.36hNSlvg` に保存しました。台帳全体は対象786件のうち等価合格144件、部分検証10件、未達632件です。汎用オブジェクトのインスタンス化、型付き参照の解決、ファイル単位のローカルスコープは残っています。

### backlog-network-47aff51c222c-1

REPL のコメント処理で、字下げモードにある複数行コメントの終端行を空行として扱い、波括弧内では後続コードの UTF-16 列を保持します。固定上流 `PreLexerTests.cs` の8件を同じバイナリで個別実行し、コメント除去後の行、空白、引用符内のコメント記号と波括弧を照合しました。上流プリレクサが後段で挿入するセミコロンは、行境界を直接処理する実装の観測点に合わせて正規化しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 159件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-repl-comment-cases.1rP2Az0a` にあります。台帳は対象786件のうち等価合格152件、部分検証10件、未達624件です。字下げと複数段のアウトデントを扱うプリレクサケースは残っています。

### backlog-network-6b71b3744382-1

REPL の最初の有効行を字句処理で確認し、字下げされている場合に専用診断を返します。固定上流 `PreLexerTests.cs` の3件を同じバイナリで個別実行し、空入力、先頭行の通過、初期字下げでの拒否を照合しました。STM32H743 の `using` を最初の有効行で字下げした CLI 入力も、2行目を指す診断で拒否しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-prelexer-first-line.aEsZ3BA2
target/release/penv check-platform "$e/initial-indent.repl" --platform-root ../renode
```

最後のコマンドは構文エラーで終了します。`cargo test` 162件、固定 STM32H7 REPL 7構成、既存の構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。台帳は対象786件のうち等価合格155件、部分検証10件、未達621件です。段階的な字下げと EOF での境界ケースは、上流が文字列に挿入する区切りと現行パーサーが内部で扱う属性境界の対応を検証する処理が必要です。

### backlog-network-131f440804a5-1

REPL の字下げ境界判定を `init:` と字下げ付きオブジェクト属性で共有し、オブジェクト属性間の空行を許容しました。固定上流 `PreLexerTests.cs` の残り7件を個別テストとして移植しました。初回は明示波括弧の終端でテスト用レンダラーが閉じ波括弧を一つ余分に生成したため、有効な字下げレベルを保持するよう修正しました。

7件とも上流出力と一致しました。テスト用レンダラーは実装と字下げ判定を共有しますが、EOF、空行、明示波括弧が混在する境界を本番パーサーからすべて観測する仕組みが残るため、台帳では部分検証です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 169件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果、初回失敗と再実行のログ、終了状態は `~/.local/state/penv-dispatch/evidence/h7-indent-boundaries.Mupqb4ab` に保存しました。対象786件のうち等価合格155件、部分検証17件、未達614件です。

### backlog-network-6af6bb158270-1

REPL の構築前に、空宣言と型の重複宣言を診断します。同じ宣言内の `init:` と `init add:` も重複として診断します。固定上流 `VerificationTests.cs` の3件を個別テストとして先に移植し、同じバイナリで各診断と該当行を照合しました。空の追記 `name: {}` は構文として受理し、上流と同じく構築時に空宣言として拒否します。単独の `init add:` を実行する処理と型・プロパティの一般的な検証は残っています。以前の字下げ7件は、本番の境界を観測する仕組みが必要なため部分検証のままです。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 172件、固定 STM32H7 REPL 7構成、既存の構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-declaration-verification.qM3fX7GY` に保存しました。台帳は対象786件のうち等価合格158件、部分検証17件、未達611件です。

### backlog-network-b374c0eff2fa-1

REPL の宣言で引用符付きエイリアスを解析し、登録の省略と明示的な `@ none` を別々に診断します。固定上流 `VerificationTests.cs` の2件を同じバイナリで実行し、同じ入力で診断の種類と該当行を照合しました。正当なエイリアスを実行時に公開する上流ケースは、REPL 宣言名からバス上の公開名を分離する名前解決が必要なため未達です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 174件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-alias-validation.4RXZtf3f` に保存しました。台帳は対象786件のうち等価合格160件、部分検証17件、未達609件です。

### backlog-network-aea5570f5fc1-1

REPL は同じ宣言内の重複属性を、追記の合成や周辺機器の構築より前に診断します。固定上流の `ShouldFailOnDoubleAttribute` で、同じ入力の2回目の属性を指す診断を照合しました。後続の追記が同名の属性を設定する場合にも元の重複を検出し、既存テストで正常な追記の上書きも確認しました。NVIC の同一宣言内の重複は、モデル固有の診断から属性名と行を示す共通診断に変わります。属性の存在・可書性・型の検証にはモデルの属性情報が必要です。正当なエイリアスの実行時公開には、宣言名とバス上の名前を分ける処理が残っています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 176件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-property-validation.E48XqaDp` にあります。対象786件のうち等価合格161件、部分検証17件、未達608件です。

### backlog-network-3d3a3ae16697-1

SystemBus の未マップアクセス方針とタグ単位の例外を REPL から設定できます。固定上流 `SystemBusTests.cs` の6件を個別テストとして同じバイナリで実行し、同じアドレスへのバイト読み書き、タグ既定値、アドレス例外を照合しました。STM32H743 の CLI プローブは初回に上流とローカルの `sysbus:` 追記が重複扱いになったため、両方を読み込み順に適用するよう修正しました。再実行ではタグ値 `0x42`、タグなしの `0`、例外タグのアドレスエラーを確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-bus-unhandled.IKQerYGw
target/release/penv run --script "$e/read.resc" --platform-root ../renode
target/release/penv run --script "$e/fault.resc" --platform-root ../renode
```

最後のコマンドは意図したアドレスエラーで終了します。`cargo test` 182件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。対象786件のうち等価合格167件、部分検証17件、未達602件です。既定未マップ方針の差、上流の報告ログ、タグ分割時の例外保持は残っています。UART の失敗文字列群は固定 Tock 構成を使い、正規表現と失敗文字列の登録解除も必要なため、今回の検証は SystemBus に切り替えました。

### backlog-network-863d546a9dff-1

SystemBus の子タグ追加で親範囲を分割し、左右に残る親タグの既定値と例外方針を保持します。タグは上流と同じくアクセスの開始アドレスで選びます。複数の既存範囲にまたがる追加は拒否し、既存タグを保持します。固定上流 `ShouldPreserveThrowExceptionWhenSplittingTag` の同じ入力で、左右の親範囲の例外と子範囲の読み取り成功を確認しました。STM32H743 の CLI でも同じ3観測をワード読み取りで確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-bus-tag-split.NLpdOJdr
for address in 0x50000000 0x50000010 0x500000f0; do
    target/release/penv run --script "$e/$address.resc" --platform-root ../renode
done
```

CLI の期待終了状態は順に 1、0、1 です。`cargo test` 184件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。台帳は対象786件のうち等価合格168件、部分検証17件、未達601件です。上流既定の未マップアクセス方針、タグの階層名を用いた報告、周辺機器からのアドレス以外のバス例外の伝播が残っています。

### backlog-network-dc6513219158-1

SystemBus の RAM 一括読み書きを実装しました。書き込みは RAM 範囲との交差部分に適用し、読み取りでは穴をゼロで埋めます。書き込みと重なる排他アクセス予約を解除します。固定上流 `SystemBusTests.cs` の6件を同じバイナリで実行し、同じアドレス、RAM 範囲、入力バイト列、期待値を照合しました。固定フィクスチャが同一 RAM を二重登録する5件では、各ケースが片側の範囲だけに触れるため、独立した同サイズ RAM で同じ観測点を再現しています。STM32H743 の CLI でも RAM 終端を越える8バイトの読み書きで、末尾2バイトがゼロになることを確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-bus-memory-bytes.fzZp4oFY
target/release/penv run --script "$e/bytes.resc" --platform-root ../renode
```

`cargo test` 191件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。上記にケース別結果と終了状態を保存しました。台帳は対象786件のうち等価合格174件、部分検証17件、未達595件です。`ShouldReadBytes` の残り3構成にはバイト幅の周辺機器転送が必要です。今回の CLI コマンドは RAM 転送に限り、上流が `onlyMemory` 指定時に行う周辺機器交差の診断と RAM の別名登録は残っています。

### backlog-network-d749d8e280de-1

SystemBus の `FindBytes` を実装しました。隣接する RAM を連続した範囲として検索し、バッファー境界をまたぐ一致を拾います。未マップの穴は検索範囲から外します。固定上流 `ShouldFindBytePattern` と同じ 1 MiB RAM 二領域とバイト列で、全一致位置を照合しました。STM32H743 の CLI プローブでは、初回の RAM アドレスが LTDC と重なったため空き範囲に移し、`0x54000000` と `0x540ffffc` を確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-bus-find-bytes.vgeb8aAf
target/release/penv run --script "$e/find.resc" --platform-root ../renode
```

`cargo test` 193件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記にあります。台帳は対象786件のうち等価合格175件、部分検証17件、未達594件です。CPU コンテキスト別の検索と、周辺機器を含む一括読み書きの上流ケースは残っています。

### backlog-network-e45b5c638e96-1

REPL から構築した SystemBus の既定方針を上流の `Report` に合わせ、未マップのデータ読み取りでゼロを返します。REPL を使わない raw/ELF 実行と命令フェッチの範囲チェックは従来どおりです。固定上流 `ShouldReturnZeroAtNonExistingDevice` と同じアドレス `0xABCD1234` でバイト読み取りを検証しました。STM32H743 の CLI でも既定設定のゼロと、明示した `ThrowExceptionIfNotTagged` の例外を照合しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-default-bus-access.T5HgC1PK
target/release/penv run --script "$e/default.resc" --platform-root ../renode
target/release/penv run --script "$e/strict.resc" --platform-root ../renode
```

最後のコマンドは意図したアドレスエラーで終了します。`cargo test` 195件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。台帳は対象786件のうち等価合格176件、部分検証17件、未達593件です。上流の報告ログと、周辺機器を含む一括転送は残っています。

### backlog-network-5d68e000c930-1

SystemBus の `ReadBytes` と `WriteBytes` は、RAM とバイト幅アクセスを実装する周辺機器を一つの転送で扱えます。未マップ区間の読み取りはゼロで、書き込みはスキップします。`onlyMemory=true` では周辺機器との交差を転送前に拒否します。固定上流 `ShouldReadBytes` の4構成を一括実行し、バイト周辺機器を含む残り3構成を等価合格に更新しました。テスト用周辺機器は上流モックと同じオフセット別の書き込みと読み戻しを再現します。STM32H743 の CLI では公開上流 REPL の USART2 に `[34, 12]` を書いて読み戻し、同じ範囲のメモリー専用読み取りが拒否されることを確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-bus-mixed-bytes.n7OGEM8c
target/release/penv run --script "$e/normal.resc" --platform-root ../renode
target/release/penv run --script "$e/memory-only.resc" --platform-root ../renode
```

最後のコマンドは意図した診断で終了します。`cargo test` 198件、STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。上記にケース別結果と終了状態を保存しました。台帳は対象786件のうち等価合格179件、部分検証17件、未達590件です。CPU コンテキスト別登録と、バイト幅を持たない周辺機器の上流アクセス幅変換は残っています。

### backlog-network-b18e8b6d9069-1

REPL の複数 RAM 登録で、一つの記憶領域を複数のバスアドレスに割り当てます。`sysbus UnregisterFromAddress` は指定した RAM 登録だけを解除します。固定上流 `Should Unregister Only The Requested Range` の `0x800` と `0x4000` の両構成を移植し、先頭に書いた値を末尾の別名から読めること、中央の解除済み範囲がゼロを返すことを照合しました。CLI の構築に必要な Cortex-M7 宣言を加え、CPU は実行せず、固定上流と同じバス操作を実行しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-ram-aliases.0iTbYDsD
for size in 0x800 0x4000; do
    target/release/penv run --script "$e/$size.resc" --platform-root ../renode
done
```

`cargo test` 200件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。台帳は対象786件のうち等価合格180件、部分検証17件、未達589件です。対象を RAM に限定しており、周辺機器の動的登録解除と CPU コンテキスト別登録は残っています。

### backlog-network-1f7c0ba5f3a3-1

定数 MMIO 周辺機器を REPL の明示範囲で登録できるようにし、範囲サイズの検証を RAM と共有しました。固定上流 `ShouldFindAfterRegistration` と `ShouldFindAfterManyRegistrations` を同じバイナリで実行しました。上流モックのオフセット0での応答を定数 MMIO で再現し、同じ登録範囲・ワード読み取り・戻り値を照合しました。構築に必要な CPU と RAM はバス操作に参加しません。上流の `ShouldFindAfterManyRegistrationsAndRemoves` は Ignore 指定で、周辺機器の動的登録解除が必要なため台帳に未達として残しています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 202件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-sysbus-lookup.hI7esteD` にあります。台帳は対象786件のうち等価合格182件、部分検証17件、未達587件です。

### backlog-network-3763ff0f38c4-1

`sysbus UnregisterFromAddress` で定数 MMIO の指定登録を解除できます。登録範囲内の開始アドレスからオフセットを求めるため、非整列の登録でも周辺機器にワード読み取りを渡します。固定上流の Ignore 指定ケース `ShouldFindAfterManyRegistrationsAndRemoves` は、100登録、16 KiB の上限、各登録の10%解除条件、全アドレスの照合を移植しました。上流が固定していない乱数はローカルの固定シードを使います。初回は解除済みの非整列アドレスで整列エラーが出たため、範囲全体が未マップならバスの未マップ方針を適用するよう修正しました。STM32H743 の CLI では、非整列の1バイト登録にワード読み取りを行い、解除前の周辺機器応答と解除後のゼロを確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-sysbus-unregister-mmio.3EX3acF1
target/release/penv run --script "$e/mmio.resc" --platform-root ../renode
```

`cargo test` 203件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果、初回失敗、最終ログと終了状態は上記に保存しました。台帳は対象786件のうち等価合格183件、部分検証17件、未達586件です。定数 MMIO と RAM 以外の解除、CPU コンテキスト別登録、周辺機器が返すコマンド例外の伝播は残っています。

### backlog-network-66faec789434-1

STM32 TIM の周期処理で、経過した仮想ステップからクロックサイクルを求める処理を共有しました。固定上流 `LimitTimerTests.cs` の4件を同じバイナリで実行し、カウントアップとダウンの単調性、上限前後での値の範囲を照合しました。周波数、初期値、上限と仮想秒の刻みは上流と同じです。STM32 TIM は ARR を含めてカウントするため、折り返しとイベント発火の厳密な時刻を観測する残り5件は、上流 LimitTimer と STM32 の周期およびイベント通知の対応を検証する必要があります。SystemBus の周辺機器コマンド例外も、周辺機器の戻り値とバスの例外型に共通経路が必要なため残しています。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
base=~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr/shell
bash tests/stm32h7-tim8-smoke.sh "$base" timer1
bash tests/stm32h7-tim8-smoke.sh "$base" timer8
```

`cargo test` 207件、固定 STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button と TIM1/TIM8 の実 ELF 回帰が通過しました。ケース別結果、ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-limit-counter.HiJSBsgZ` にあります。台帳は対象786件のうち等価合格187件、部分検証17件、未達582件です。

### backlog-network-9ee4c5869953-1

STM32 TIM の仮想ステップ処理で、割り込み許可中の更新イベント回数を確認できます。固定上流 `LimitTimerTests.cs` の4件を同じバイナリで実行し、1 MHz で 10 µs と 10 MHz で 1 µs の両方で2イベントと現在値2、1 Hz では割り込み許可の有無に応じた通知を照合しました。上流の下降周期は STM32 の `ARR=limit−1`、現在値は `CNT+1` に対応させています。方向切替ケースは上流がゼロ到達、STM32 TIM がゼロからのアンダーフローで通知するため、汎用 LimitTimer の方向変更セマンティクスが必要です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-rcc-timer-robot.sh "$(mktemp -d)"
base=~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr/shell
bash tests/stm32h7-tim8-smoke.sh "$base" timer1
bash tests/stm32h7-tim8-smoke.sh "$base" timer8
```

`cargo test` 211件、STM32H7 REPL 7構成、構文診断3件、RCC/TIM1 の3件、公開 Zephyr Blinky・Button と TIM1/TIM8 の実 ELF 回帰が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-limit-events.6PIr5DnN` に保存しました。対象786件のうち等価合格191件、部分検証17件、未達578件です。

### backlog-network-5966ef07b946-1

固定上流 `ShouldSwitchDirectionProperly` の100 Hz、上限100000、上昇2秒、方向切替、下降2秒を STM32 TIM で再現しました。ゼロ到達の通知は既存の `CCR1=0` の比較割り込みを使用します。切替で CNT が保持され、到達直前に通知がなく、到達時に比較割り込みが立つことを照合しました。STM32 の更新割り込みはアンダーフローで発火するため、比較割り込みを観測点に選びました。固定 `LimitTimerTests.cs` の台帳9件はすべて等価合格です。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-rcc-timer-robot.sh "$(mktemp -d)"
base=~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr/shell
bash tests/stm32h7-tim8-smoke.sh "$base" timer1
bash tests/stm32h7-tim8-smoke.sh "$base" timer8
```

`cargo test` 212件、STM32H7 REPL 7構成、構文診断3件、RCC/TIM1 の3件、公開 Zephyr Blinky・Button と TIM1/TIM8 の実 ELF 回帰が通過しました。ケース別結果と終了状態は `~/.local/state/penv-dispatch/evidence/h7-limit-direction.xwyONkQv` に保存しました。台帳は対象786件のうち等価合格192件、部分検証17件、未達577件です。共通 ComparingTimer の上限を超える比較値の拒否は STM32 TIM の CCR 書き込みと意味が異なるため、別の未達項目です。

### backlog-network-3902aaeb6c9e-1

STM32H7 GPIO からタイマーへの接続で、接続先の保持と入力上限の検証を共通化しました。固定上流 `GPIOTests.cs` の4件を同じバイナリで実行し、接続先と入力番号、未接続の空リスト、入力上限、および不正入力の診断全文を照合しました。上流のモック受信先には5入力、実 STM32H743 の TIM2 には4入力を使用しました。実 REPL に追記した入力2は受理され、入力10は受信先の入力数を示す診断で拒否されました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-gpio-endpoints.KfA5tzQp
target/release/penv check-platform "$e/channel-2.repl" --platform-root ../renode
target/release/penv check-platform "$e/channel-10.repl" --platform-root ../renode
```

最後のコマンドは意図した診断で終了します。`cargo test` 216件、STM32H7 REPL 7構成、構文診断3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。対象786件のうち等価合格196件、部分検証17件、未達573件です。GPIO の動的切断は残っています。ComparingTimer の上限を超える比較値の拒否は、書き込み可能な STM32 TIM の CCR と契約が異なるため、汎用比較タイマーの経路が必要です。

### backlog-network-12ea45ec088d-1

`sysbus UnregisterFromAddress` で STM32H7 TIM を解除すると、そのタイマーへの GPIO 入力接続と PWM 参照を削除します。固定上流 `GPIODetachingTests.cs` の番号付き接続2件を同じバイナリで実行しました。上流モックの代わりに STM32H7 GPIO と TIM を使い、MMIO 領域のサイズに合わせて登録アドレスを変更しました。入力番号と接続関係を維持し、解除対象の端点が空になり、他の端点が残ることを照合しました。STM32H743 の CLI では TIM2 の解除後にその ARR がゼロ、TIM3 の ARR が元の値を返しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-rcc-timer-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-gpio-detach.9xXurQ8I
target/release/penv run --script "$e/unregister.resc" --platform-root ../renode
```

`cargo test` 218件、STM32H7 REPL 7構成、構文診断3件、RCC/TIM1 3件と公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。台帳は対象786件のうち等価合格198件、部分検証17件、未達571件です。中継周辺機器の入出力を同時に切断する汎用 GPIO グラフと、外部接続・別マシンの破棄を扱うケースは残っています。
