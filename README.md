# penv

`penv` は、Renode のファームウェア実行機能を Rust で段階的に実装する CLI エミュレーターです。ベアメタルの RV32、RV64、および Cortex-M ファームウェアを実行できます。外部クレートは不要です。

## ビルドと実行

```sh
cargo build --release
target/release/penv run --stats firmware.elf
target/release/penv run --raw --ram-mib 1 --max-steps 1000000 firmware.bin
target/release/penv run --help
```

ELF はリトルエンディアンの静的な RISC-V ELF32 と ELF64 の実行ファイルを受け付けます。リンカースクリプトでロード対象の仮想アドレスを `0x80000000` から配置し、エントリーポイントを実行可能セグメント内に置いてください。raw イメージは同アドレスからロード・実行します。主 RAM の既定値は 16 MiB です。別アドレスにリンクした ELF では、たとえば `--map-ram 0x1000:0x40000` で RAM を追加できます。このオプションは複数回指定でき、容量の単位はバイトです。

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

## RV64

リトルエンディアンの静的な RISC-V ELF64 は、クラスから RV64 を自動選択します。セグメントは物理アドレスにロードされます。起動時はページ変換が無効です。実行可能セグメントの仮想エントリーポイントは、同じ RAM を指す別名で起動するか、対応する物理アドレスに変換されます。raw の RV64IM イメージには `--raw --rv64` を指定します。`--map-ram` と `--load-binary BASE:PATH` には 4 GiB 以上のアドレスも指定できます。後者は DTB、カーネル、initramfs などをファームウェアと一緒にロードします。

```sh
target/release/penv run --platform ../renode/platforms/cpus/sifive-fu740.repl \
  --platform-root ../renode --stats firmware64.elf
```

`CPU.RiscV64` は RV64IMAFDC、Zicsr、Zifencei、M/S/U モード、PMP、Sv39 と Sv48 に対応します。REPL の ISA 指定で Zba、Zbb、Zbc、Zbs、Zicond、Zacas を選択できます。SiFive、Kendryte、PolarFire、StarFive の REPL から RAM とその別名、ハート、CLINT、PLIC、SiFive UART または NS16550 を構成できます。CLINT の REPL に `frequency` がある場合、各ハートが既定の 100 MIPS で進む仮想時間に合わせて `mtime` を更新します。複数の UART がある場合は `--uart-device NAME` で入力先を選択できます。REPL の `Tag` と `SilenceRange` で指定した範囲では既定値を返します。その他の未実装アドレスへのアクセスはゲストの例外になります。

### BeagleV StarLight

Renode の `scripts/single-node/beaglev_starlight.resc` と同じ OpenSBI、U-Boot、Linux 5.10.6、Buildroot を起動します。ファイル名に含まれる SHA-1 を照合してから実行します。RESC 内のシンボル指定はデバッガー用メタデータとして扱い、ゲスト RAM には ELF と FIT をロードします。

```sh
set -e
cargo test
cargo build --release
resc=../renode/scripts/single-node/beaglev_starlight.resc
assets=$(mktemp -d)
rg -o 'https://[^[:space:]]+' "$resc" | while IFS= read -r url; do
  name=${url##*/}
  curl -fLsS --retry 2 "$url" -o "$assets/$name"
  printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum -c -
done
target/release/penv run --script "$resc" --platform-root ../renode \
  --asset-dir "$assets" --uart-device uart3 --uart-stdin \
  --max-steps 14000000000 --stats
```

`buildroot login:` に `root`、`Password:` に上流デモのパスワード `starfive` を入力します。シェルで `id -u` と `uname -m` を実行すると、それぞれ `0` と `riscv64` を確認できます。`--uart-stdin` は標準入力のバイトを実行中の UART に送り、改行のないプロンプトも出力します。

UART 単体、2 台直結、スイッチ経由の検証は同じ公開イメージを使います。上の手順で取得した `$assets` を指定してください。

```sh
python3 scripts/verify-starlight.py "$assets"
python3 scripts/verify-starlight.py "$assets" --ports 30451 30452
python3 scripts/verify-starlight.py "$assets" --ports 30454 30455 --switch-port 30453
```

各モードでブートメッセージ、UART ログイン、`id -u` の `0` と `uname -m` の `riscv64` を確認します。直結モードではゲストの UDP ポート 2 つ、スイッチモードでは追加でスイッチ用ポート 1 つを指定します。ゲストの `eth0` に上流 Robot と同じ MAC アドレス、静的 IPv4 アドレス、MTU 440 を設定します。56 バイトと 3200 バイトの ICMP ペイロードを両方向に各 10 回送り、各ゲストの `/proc/interrupts` で `eth0` の割り込み増加を確認します。3200 バイトのケースは IP フラグメントを使用します。ログは `target/starlight-verification/` に保存します。

スイッチモードは上流 `tests/platforms/BeagleV-StarLight.robot` の `CreateSwitch` と、各 `sysbus.ethernet` の `connector Connect` に対応する配線です。スクリプトは `penv switch 30453 30454 30455` を起動し、ゲストをそれぞれ `--ethernet-loopback 30454:30453` と `--ethernet-loopback 30455:30453` で接続します。スイッチは送信元 MAC を学習し、宛先が分かるユニキャストは該当ポートへ、未学習の宛先やブロードキャストは他のポートへ転送します。UDP はすべて `127.0.0.1` に限定します。

ゲストは起動中に DHCP Discover を送信します。DHCP 応答のない検証リンクでは、3 回の Discover と `udhcpc: no lease, failing` を経て `buildroot login:` に到達します。65 億ステップで切ると DHCP 再試行中に上限へ達します。ゲスト内の DHCP サーバー起動は `FAIL` を報告します。

BeagleV MAC は通常記述子の TX/RX DMA、チェーンとリング折り返し、RX フレーム長と FCS、割り込みマスクと PLIC 配線をモデル化します。回帰テストでは RX の複数記述子と 2 バッファーへの転送、記述子不足からの復帰、完了 IRQ の抑止、PLIC の claim・完了、TX バスエラー後のリセットを検証します。

Linux の通信試験では、PLIC の出力が下がってもゲストが supervisor 外部割り込みの処理を繰り返すことがありました。`mip` の CSRRS/CSRRC は命令の戻り値に外部信号を反映し、書き戻しには `SEIP` のソフトウェア側の値を使うよう修正しました。[RISC-V Privileged Architecture v1.10](https://riscv.org/wp-content/uploads/2017/05/riscv-privileged-v1.10.pdf)

公開イメージによる無人のペア検証では、直結とスイッチ経由の両方でログインとコマンド実行に成功し、それぞれ 4 ケースすべてが 10/10 応答、パケット損失 0% でした。スイッチ経由の `eth0` IRQ カウンターは A が 3 から 109、B が 3 から 108 に増加しました。3 ポートの UDP 疎通試験でフラッディング、学習済みユニキャスト、学習済みの同一入出力ポートへの転送、未登録ポートの除外を確認しました。`cargo test` は 45 件通過しました。

スイッチのポートは起動時に固定し、MAC 学習表は最大 1024 件です。動的な着脱、スイッチのポート単位のプロミスキャスモード、Renode Monitor 構文の実行、複数マシン間の仮想時刻同期、外部ネットワーク接続は残課題です。MAC 側のマルチキャストハッシュの厳密な照合、チェックサムオフロード、MAC 送信無効時の TX 先読みも未実装です。上流 Robot の外部 PMP 付きケースは未検証です。性能最適化と比較計測は互換性実装が揃った後に行います。

共通 CLI の回帰確認では、上記の Murax デモが UART に `Ant` を出力し、LiteX VexRiscv SMP デモがログイン後の `cat /proc/cpuinfo` で 4 ハートを出力しました。どちらも指定ステップ上限で終了します。

## Cortex-M0 / Cortex-M0+

REPL の `CPU.CortexM` で `cpuType: "cortex-m0"` または `cpuType: "cortex-m0+"` を指定すると、限定的な Thumb 実行系を使用できます。`--raw` はイメージを `vectorTableOffset`（既定値 0）に読み込み、ベクターテーブルから初期 SP と PC を設定します。整数演算、分岐、即値オフセットでのロード・ストア、LiteX UART への出力を確認済みです。`BKPT` で実行を終了します。32 ビットの BL と、PUSH・POP、SP 相対のロード・ストアにも対応します。ARM EABI5 の静的 ELF32 は物理ロードアドレスを使用します。明示的なベクターアドレスがない場合は、ロードされる割り当て済みセクションの最小物理アドレスを使い、該当セクションがなければロードセグメントの先頭を使います。リセット時の SP と PC はそのベクターテーブルから取得します。起動後は VTOR への書き込みで、例外用のベクターテーブルをマップ済みの 128 バイト境界に移動できます。SVC はベクターテーブル経由でハンドラーを呼び出し、MSP を使って元の実行状態を復元できます。SysTick は実行ステップをクロックとして動作し、CPSID/CPSIE で割り込みをマスクできます。NVIC は 32 本の外部 IRQ の有効化、保留、優先度を管理し、REPL で LiteX UART を接続できます。高優先度の外部 IRQ または SysTick は実行中のハンドラーに割り込み、MSP で元のハンドラーに復帰できます。LDM/STM、DSB/DMB/ISB、および MSP、PRIMASK、PSR の MRS/MSR にも対応します。CONTROL.SPSEL でスレッドのスタックを切り替え、PSP を使った例外フレームから復帰できます。PendSV は ICSR から保留・解除でき、SHPR3 の優先度に従って配送されます。SVC のエスカレーション、CONTROL.nPRIV、その他の 32 ビット Thumb 命令は未対応です。

Cortex-M の REPL で `UART.STM32F7_USART` を複数配置し、各 USART を NVIC に直接接続するか、STM32F4 EXTI や CombinedInput を経由して接続できます。受信は USART と受信器を有効にした後に届けてください。標準入力用のファイルは、STM32 USART だけを配置した場合は最初に宣言した USART へ送られます。送信は即時で、DMA 配線は未対応です。

## 現在の実装範囲

RV32I 基本整数命令、M 拡張、Zifencei を実装しています。REPL で `rv32ima` または `rv32ima_zicsr_zifencei` を指定すると、共有 RAM 上の A 拡張命令を使用できます。`rv32imc` と `rv32imac` では RV32C の整数圧縮命令に対応します。`cycle`、`instret`、`time` と各上位 32 ビットを読み取れます。CLINT がある場合の `time` は `mtime` に従います。Zicsr の命令と、マシンモードの CSR の一部、および `ECALL`・`EBREAK` からの `MRET` を実装しています。`CPU.RiscV32` ではユーザーモードへの移行とマシンモードへのトラップ、16 エントリーの PMP に対応しています。スーパーバイザーモードでは同期例外の委譲と `SRET`、32 ビット物理アドレス範囲の Sv32 ページ変換に対応しています。変換はハートごとに 64 エントリーをキャッシュし、SFENCE.VMA でそのハートのキャッシュを無効化します。マシンモードから設定したスーパーバイザー割り込みを委譲できます。3 コンテキスト以上の PLIC、その他の ISA 拡張、REPL のその他の構文とデバイス、起動コマンド以外の RESC 構文、ネットワーク、複数ノードは未対応です。標準構成の UART は単一の送信レジスターを提供します。

RV32I の仕様は [RISC-V ISA マニュアル](https://docs.riscv.org/reference/isa/unpriv/rv32.html) を参照してください。
