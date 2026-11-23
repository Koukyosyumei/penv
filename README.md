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

- `cargo test` はユニットテスト 42 件、CLI テスト 5 件が通過しました。STM32H7 の DMA 記述子と LiteX の CSR32 スロットを仮想スイッチで接続し、両方向のフレームと受信データを確認しました。SPI のテストは RCC ゲート、リセット、DMAMUX の要求番号、受信 DMA、NVIC、サスペンドを確認します。
- STM32H7 の上流単一ノード RESC は `aa62fb3` で `stm32h7.repl:193` の `SPI.STM32H7_SPI`、`7d09525` で `stm32h7.repl:199` の `I2C.STM32F7_I2C` により停止しました。現行コードは両方の宣言を通過し、`stm32h7.repl:219` の `DMA.STM32DMA2D` で停止します。上流複数ノード RESC は `emulation CreateCANHub` で停止します。
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

検証スクリプトは同じ上流 RESC のリセットマクロを使い、REPL の参照だけを生成した診断構成に差し替えます。診断構成は上流 REPL から起動に必要な宣言を抽出し、Cortex-M7、480 MHz の SysTick、USART2 の EXTI 配線を使います。DBGMCU ID と CAN CCFG は定数応答です。システムフラッシュの REPL 初期化スクリプトは省いています。ELF は上流 URL から取得し、SHA-1 を検証します。キャッシュ済みファイルを使う場合は `PENV_ASSET_DIR` を指定できます。

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc tests/stm32h7-zephyr-smoke.sh
```

スクリプトは USART2 の出力と Penv のログを一時ディレクトリに保存します。公開 ELF の起動バナーとシェルプロンプトに加え、実行ステップ 850,000 で入力した `help` への応答、1,300,000 で入力した `kernel version` への応答を検証します。実行は 2,000,000 ステップの上限で終了します。

```text
*** Booting STM32H7 Renode Platform Demo 6607bdd711f4 ***
demo:~$ help
Available commands:
demo:~$ kernel version
Zephyr version 4.4.99
```

変更前の診断実行で停止した `pc=0x080036b6` は、ITCM を初期化する `strd r0, r1, [r3], #8`（先頭半語 `0xe8e3`）でした。Thumb-2 のデコーダーはローカル nRF52840 担当の `fe1911b` を再利用し、Cortex-M7 の実行ログで必要になった命令を追加しました。CRC の演算モデルはローカル STM32F0 の実装を再利用しています。起動時のバスアクセスに合わせて、RCC、SysTick、RNG、TIM3、CRC、QuadSPI の初期化・識別・SFDP と受信送信割り込み、FDCAN の設定レジスターを配線しました。

残課題は未変更の上流 REPL のロード、I2C 配下のセンサー、QuadSPI のフラッシュデータ操作、CAN フレーム送受信です。診断実行では LSM6DSO の初期化失敗と外部フラッシュのマウント失敗 `err=-45` が出力されます。MPU とキャッシュはファームウェアの初期化レジスターに応答する範囲です。

## STM32F407 Discovery と FreeRTOS

`penv/stm32f407` は `3686ab4` を起点に、Cortex-M4 の整数命令を既存の
Thumb-2 実装で実行します。NVIC、SysTick、SVC、PendSV、PSP/MSP の処理も共用します。
`UART.STM32_UART` は F4 の SR/DR/CR1 レイアウトを使用し、既存の STM32 USART の
入出力と割り込み配送を共用します。

上流の `stm32f4_discovery.resc` は Contiki 用で、`PerformanceInMips` の解釈で停止します。
ボード定義だけを読み込む場合の阻害点は下記の「未変更の上流 REPL」で再現できます。
FreeRTOS 検証用の REPL には、上流を参照して UART4（`0x40004c00`、IRQ 52）、
TIM2（`0x40000000`、IRQ 28）、GPIOC、NVIC の配線を含めました。メモリー容量は STM32F407VG の 1 MiB フラッシュと
128 KiB メイン SRAM、SysTick はリセット時の HSI 16 MHz を使用します。
ファームウェアは PC10/PC11 の AF8 を設定し、UART4 から 115200 baud 設定で送信します。
[`stm32f4_discovery.resc`](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/scripts/single-node/stm32f4_discovery.resc)
[`stm32f4.repl`](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/stm32f4.repl)
[STM32F407VG データシート](https://www.st.com/resource/en/datasheet/stm32f407vg.pdf)

最初の TIM2 割り込みでは、FreeRTOS の `xQueueSendFromISR` が IRQ 優先度を
検証するために SCB_AIRCR（`0xe000ed0c`）を読み、未マップアクセスで停止しました。
NVIC モデルはリセット値 `0xfa050000` を返し、`PRIGROUP=0` を公開します。
[STM32 Cortex-M4 プログラミングマニュアル PM0214、4.4.5](https://www.st.com/resource/en/programming_manual/pm0214-stm32-cortexm4-mcus-and-mpus-programming-manual-stmicroelectronics.pdf)

### ビルドと実行

Git、Clang、LLD、Rust/Cargo が必要です。下記は Linux x86_64 上で Clang/LLD 15.0.7 と
ホストリンカー GCC を使った実行例です。ビルドスクリプトは公式 FreeRTOS-Kernel
V11.1.0（`dbf70559b27d39c1fdb68dfb9a32140b6a6777a0`）を取得し、公式
`portable/GCC/ARM_CM3` の整数ポートを `-mcpu=cortex-m4 -mfloat-abi=soft` で
リンクします。起動コード、メモリー配置、タスクは
`tests/fixtures/stm32f407-freertos/` にあります。
[FreeRTOS-Kernel V11.1.0 の ARM_CM3 ポート](https://github.com/FreeRTOS/FreeRTOS-Kernel/tree/dbf70559b27d39c1fdb68dfb9a32140b6a6777a0/portable/GCC/ARM_CM3)

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/stm32f407-freertos-smoke.sh target/stm32f407-freertos

# 生成済み ELF を同じ入力で直接実行
work=target/stm32f407-freertos
target/release/penv run --script "$work/boot.resc" --max-steps 36000 \
    --uart-event "24000:$work/uart-x.txt" --uart-event "30000:$work/uart-yz.txt"
```

期待する UART4 出力は以下のとおりです。スモークテストは標準出力を全行照合し、
命令上限で終了コード 2 になることを確認します。

```text
STM32F407 FreeRTOS V11.1.0
PWR ready OK
CAN loopback OK
SPI IRQ and DMA OK
I2C echo 01 23 and NACK OK
task A tick=01
task B tick=03
TIM2 queue=1
task A tick=05
task B tick=07
TIM2 queue=2
task A tick=09
task B tick=11
TIM2 queue=3
UART4 echo=X
UART4 echo=Y
UART4 echo=Z
```

- 両タスクは初回出力前と各出力後に `vTaskDelay` でブロックします。SysTick で
  tick が進み、解除されたタスクへ PendSV が切り替えます。各タスクは出力前に tick 値を
  検査し、A/B それぞれ 3 回の実行を確認できます。
- TIM2 は 4 ms 周期の更新割り込みを 3 回発行します。TIM2 と UART4 RXNE の ISR は
  `xQueueSendFromISR` で同じ静的キューに通知し、受信タスクが `xQueueReceive` で
  取り出します。スモークテストは `X` と `YZ` を 2 回に分けて注入し、UART4 の
  応答順を照合します。一時 REPL で IRQ 28 と IRQ 52 の配線を個別に外した確認では、
  それぞれに対応する出力だけが消えました。
- `cargo test` はユニットテスト 53 件、CLI テスト 5 件が通過しました。既存の
  `tests/stm32h7-zephyr-smoke.sh` も通過し、USART2 で `Zephyr version 4.4.99`
  を確認しました。
- 別々の出力ディレクトリでビルドした ELF はバイト単位で一致しました。この環境での
  SHA-256 は `de1ac7adac04303e72dd99b5af83bb28ae50c22c2bf8ad0b99a12276a704a617` です。

### CAN1/CAN2 のローカル検証

検証用 REPL は上流と同じ `CAN.STMCAN` の MMIO 範囲、NVIC 配線（CAN1 は IRQ 19–22、
CAN2 は IRQ 63–66）、`master: can1` を使用します。CAN2 は CAN1 のフィルターバンクを
共有し、`CAN2SB` で割り当てを分けます。割り込み出力は TX、RX FIFO0、RX FIFO1、
SCE の順です。
[Renode の STMCAN モデル](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/CAN/STMCAN.cs)
[RM0090 の bxCAN 章](https://www.st.com/resource/en/reference_manual/dm00031020-stm32f405-415-stm32f407-417-stm32f427-437-and-stm32f429-439-advanced-arm-based-32bit-mcus-stmicroelectronics.pdf)

スモークテストの ELF はスケジューラー開始前に CAN1/CAN2 のサイレント内部ループバックを
実行します。CAN1 は ID `0x456` を受信フィルターで拒否し、ID `0x123` を FIFO0 に
受信します。CAN2 は CAN1 が設定したバンク 14 を使用し、ID `0x456` を FIFO1 に受信します。
ファームウェアは両フレームの ID、DLC、ペイロード、TX 完了、受信 IRQ と FIFO 解放、
CAN1 のスリープ通知を確認して `CAN loopback OK` を表示します。その後に SysTick、
TIM2、FreeRTOS キュー、UART4 を検証します。

### STM32F407 の PWR

上流と同じ `Miscellaneous.STM32_PWR @ sysbus 0x40007000` を登録します。モデルは
RM0090 の F407 用 `PWR_CR` と `PWR_CSR` を実装し、整列した 8/16/32 ビットアクセスを
受け付けます。VOS は 1 ビットで、VOSRDY とバックアップレギュレーターの BRR は
待ち時間なしで更新します。DBP がセットされているときに BRE を変更できます。
CWUF と CSBF は対応する状態フラグをクリアします。
[RM0090 の PWR 章、5.4 節](https://www.st.com/resource/en/reference_manual/rm0090-stm32f405-415-stm32f407-417-stm32f427-437-and-stm32f429-439-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)

スモークテストの ELF は RCC の PWREN を設定し、DBP の保護、VOSRDY と BRR の
ポーリング、BRE の解除を確認して `PWR ready OK` を表示します。続いて CAN、IRQ、
キュー、UART4 の出力を全行照合します。検証用 REPL で PWR の MMIO は常にアクセス可能です。
RCC のクロックゲートとリセット、RTC・バックアップ SRAM への DBP の伝播、
PVD・WKUP の入力、Stop/Standby 遷移、レギュレーターの安定化時間は残課題です。

### STM32F407 の SPI

上流と同じ `SPI.STM32SPI` の `STM32Series.F4` 宣言を使用します。SPI1/2/3 の
割り込みは NVIC 35/36/51 に、SPI2 の `DMAReceive` は既存の STM32 DMA モデルの
DMA1 ストリーム 3 に配線します。マスターの全二重転送は 8/16 ビットに対応します。
接続先がない SPI はゼロを受信し、検証用 REPL に接続した `SPI.SPILoopback` は送信値を返します。
[Renode の STM32SPI モデル](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SPI/STM32SPI.cs)
[RM0090 の SPI 章](https://www.st.com/resource/en/reference_manual/dm00031020-stm32f405-415-stm32f407-417-stm32f427-437-and-stm32f429-439-advanced-arm-based-32bit-mcus-stmicroelectronics.pdf)

スモークテストの ELF は SPI1 の TXE 割り込みで `0xa5` を送信し、RXNE 割り込みで
同じ値を読み出します。続けてオーバーラン割り込みを発生させ、最初のフレームを保持し、
DR、SR の順に読んでフラグが消えることを検査します。SPI2 では `0x3c` と `0xc3` が
DMA1 の完了割り込み後に SRAM に並ぶことを、SPI3 では 16 ビットの `0x12ab` が戻る
ことを確認します。すべて通過すると `SPI IRQ and DMA OK` を出力します。

SPI は転送を同期完了として扱います。SCK/NSS とビット時間、RCC のクロックゲート、
スレーブ、片方向・受信専用モード、CRC 計算、I2S、ループバック以外の SPI 接続先、
送信 DMA、DMA の CHSEL による要求の選択は残課題です。

### STM32F407 の I2C

上流と同じ `I2C.STM32F1_I2C` 宣言を使用し、イベント・エラー割り込みを I2C1 は
NVIC 31/32、I2C2 は 33/34、I2C3 は 72/73 に配線します。旧世代レジスター構成の
7 ビットマスター送受信、反復 START、STOP、アドレス NACK を扱います。ADDR は SR1、SR2
の順に読むと解除され、AF は SR1 の該当ビットへのゼロ書き込みで解除されます。
[Renode の STM32F1_I2C モデル](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b6a841a013db85/src/Emulator/Peripherals/Peripherals/I2C/STM32F1_I2C.cs)
[RM0090 の I2C 章](https://www.st.com/resource/en/reference_manual/rm0090-stm32f407-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)

検証用 REPL は `Mocks.EchoI2CDevice` を I2C1 のアドレス `0x10` に接続します。
上流の ZynqMP エコーテストの `01 23 45` を書いて反復 START から `01 23` を読む
ケースを FreeRTOS ELF に移植しました。ファームウェアはイベント IRQ で送受信し、
未接続の `0x11` へのアクセスでエラー IRQ を確認して `I2C echo 01 23 and NACK OK`
を出力します。Zedboard の 20 バイト書き込み・21 バイト読み出しも Rust テストに移植し、
最後のゼロ埋めまで確認します。
[Renode の ZynqMP エコーテスト](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/zynqmp_linux.robot#L143)
[Renode の Zedboard エコーテスト](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/Zedboard.robot#L291)

SCL/SDA の信号とタイミング、RCC クロックゲート、10 ビットアドレス、スレーブ動作、
DMA、SMBus、バス仲裁、エコー以外の接続先は残課題です。

### STM32F407 の IWDG と FreeRTOS の再起動

STM32H7 を担当する `penv/network` の `b96599c` を参照し、F407 の上流宣言
`Timers.STM32_IndependentWatchdog @ sysbus 0x40003000` を配線しました。上流指定の
32 kHz、ウィンドウなし、初期プリスケーラー値 `0x4` を使用します。KR の開始・書き込み
解除・給餌キーと PR/RLR の保護を実装し、整列した 16/32 ビットのアクセスを処理します。
満了時には Cortex-M の初期 SP と PC を元のベクターテーブルから読み直します。NVIC、
SysTick、検証で使用する周辺機器をリセットし、SRAM、フラッシュ、ロード済み ELF を保持します。
[RM0090 の IWDG 章](https://www.st.com/resource/en/reference_manual/dm00031020-stm32f405-415-stm32f407-417-stm32f427-437-and-stm32f429-439-advanced-arm-based-32bit-mcus-stmicroelectronics.pdf)

上記のスモークテストは通常起動に加え、UART の `!` を受けた FreeRTOS タスクから
約 6 ms の期限で IWDG を開始するケースを実行します。4 tick 後に給餌し、元の期限を
越えて生存したことを確認します。次の満了で再起動したファームウェアは、スタートアップが
初期化しない SRAM の値と、IWDG・SysTick・NVIC・UART のリセット状態を検査します。
`watchdog-uart.log` では通常の出力一式が次の 3 行を挟んで 2 回現れ、再起動後も
`X`、`Y`、`Z` に応答します。シミュレーター側のログは `watchdog-penv.log` です。

```text
IWDG armed
IWDG refresh held
IWDG reboot: SRAM retained
```

生成済みの ELF でリセットケースを直接実行できます。

```sh
work=target/stm32f407-freertos
target/release/penv run --script "$work/boot.resc" --max-steps 90000 \
    --uart-event "24000:$work/uart-x.txt" --uart-event "30000:$work/uart-yz.txt" \
    --uart-event "38000:$work/uart-watchdog.txt" \
    --uart-event "75000:$work/uart-x.txt" --uart-event "81000:$work/uart-yz.txt"
```

IWDG の更新待ち状態、フラッシュオプションによる自動起動、デバッグ時停止、RCC の
リセット原因フラグ、ボード全域のリセットは残課題です。

### 未変更の上流 REPL

`../renode` の `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` で確認しました。
`5fa3471` では `stm32f4.repl:10` の `MTD.STM32F4_FlashController` が最初の阻害点でした。
現在は FLASH レジスターと `optionBytes` の二つのバス登録を受理し、`flash: flash` を
消去対象のメモリーに結び付けます。モデルは ACR、キーによるロック解除、オプションバイト、
F407 の先頭 1 MiB の同期消去を扱います。FreeRTOS ELF は同じフラッシュ宣言を含む
検証用 REPL で、ACR の待ち状態の書き戻し、CR のリセット時ロック、RDP/nWRP の読み出しを
検査してからスケジューラーを開始します。セクター消去の境界とロックは Rust のテストで確認します。
[Renode の F4 フラッシュモデル](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/MTD/STM32F4_FlashController.cs)
[STM32F407 リファレンスマニュアル RM0090](https://www.st.com/resource/en/reference_manual/rm0090-stm32f405-415-stm32f407-417-stm32f427-437-and-stm32f429-439-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)

上記のスモークテストで生成した ELF を、未変更の上流ボード定義で指定します。

```sh
work=target/stm32f407-freertos
cat > "$work/upstream.repro.resc" <<'EOF'
mach create "F407_reference"
machine LoadPlatformDescription @platforms/boards/stm32f4_discovery-kit.repl
sysbus LoadELF @freertos.elf
EOF
target/release/penv run --script "$work/upstream.repro.resc" \
    --platform-root ../renode --max-steps 36000
```

`10cea9f` では `stm32f4.repl:31` の CAN、`2fb7018` では 47 行目の PWR、
`ddd976f` では 130 行目の SPI、`9348d9c` では 143 行目の I2C、
`9bdf9d3` では 165 行目の `Timers.STM32_IndependentWatchdog` で停止しました。
現在は IWDG の宣言を通過し、`stm32f4.repl:173: rcc: unsupported platform model "Miscellaneous.STM32F4_RCC"`
で終了コード 1 になります。上流 REPL での ELF 起動には RCC 以降の対応が必要です。

残課題は AIRCR の書き込み（優先度グループ変更とリセット要求）、FPU 命令と ARM_CM4F の
浮動小数点コンテキスト、上流ボード全体の読み込み、実機での確認です。フラッシュの
プログラム動作、消去の所要時間と IRQ、RDP の強制、キャッシュ動作は未実装です。
検証用 REPL では RCC の有効化書き込みを定数応答モデルで受けます。PLL、クロックゲート、
UART のビット時間や GPIO 信号線の動作は検証範囲外です。
CAN の通常モードでのノード間通信、CAN ハブ、ビット時間、バス仲裁、ACK やエラーの
生成、RCC による CAN のクロック制御は残課題です。
