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
| IIS2MDC | 上流 `IIS2MDC.robot` の正・負のベクトルを参照 Shell で照合。REPL の `MagneticFluxDensityX/Y/Z` は整数 nT。上流専用 polling ELF は診断構成で `sensor: device not ready.` を表示。RESD の時系列・GPIO トリガーと専用 ELF の再現は残件。 |
| DMA2D、LTDC | Shell から DMA2D の完了・ピクセル・IRQ 90 を照合。上流 `STM32LTDC.robot` のゼロ行テストを移植し、設定をステージした時点では ISR が 0、即時再読み込み後は ISR が `0xa`、IRQ 88 が保留。復元後に元の PPM を取得。専用 display ELF のパネル初期化と画像出力も通過。ボタン入力と走査に同期した発生タイミングは残件。 |
| PWM | 参照ボードの TIM3 CH1、GPIOB4 AF2、`pwmled0` の配線を使い、実 Shell で 25% を確認。出力はタイマー設定から算出したデューティ比。波形による測定は残件。 |
| SD | 上流 `STM32SDMMC.robot` と同じ ELF・ext2 イメージで `Disk mounted.` と `[DIR ] lost+found` を確認。カード未接続時の失敗経路も照合。 |
| 仮想 Ethernet | 上流 `nucleo_h753zi.robot` の `Should Talk Over Ethernet` と同じ echo server/client ELF を仮想スイッチに接続。診断 REPL で両ノードが起動し、サーバーで TCP 接続受理と 4 回以上の `Received and replied` を照合。クライアント側 `Received and compared … all ok` は残件。STM32H7 と LiteX の双方向フレームは既存のモデルテストで照合。 |

Ethernet の第 3 スクリプトは、`--uart-log-dir` でノード作成順に `0.uart.log` と `1.uart.log` を保存します。ディレクトリは事前に作成し、同名ファイルのない場所を指定します。echo ELF の実行で到達した Cortex-M の `CPSIE/CPSID if` と、LR を使うワード版の排他アクセスにも対応しました。
