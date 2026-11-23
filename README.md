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

### 汎用 RV64 virt の Linux 起動

[`platforms/rv64_virt.repl`](platforms/rv64_virt.repl) と [`platforms/rv64_virt.dts`](platforms/rv64_virt.dts) は QEMU 9.2 の legacy CLINT/PLIC を使う virt に対応する単一ハートの構成です。Renode 上流の `platforms/cpus/riscv_virt.repl` は RV32 ハート、M モード用 PLIC コンテキスト 1 個、UART ソース 3 を定義しています。Penv の RV64 virt では QEMU と同じ UART ソース 10、PLIC の M/S コンテキスト、512 MiB の RAM を使います。BeagleV 固有のリセットや Ethernet はこの構成に登録しません。[QEMU virt のメモリマップと DTB](https://github.com/qemu/qemu/blob/v9.2.0/hw/riscv/virt.c) [QEMU virt の IRQ 定義](https://github.com/qemu/qemu/blob/v9.2.0/include/hw/riscv/virt.h)

| 機器 | アドレス | この構成での接続 |
| --- | --- | --- |
| RAM | `0x80000000` | 起動ファームウェア、Linux、initramfs、DTB |
| CLINT | `0x02000000` | 10 MHz、hart 0 の MSIP/MTIP |
| PLIC | `0x0c000000` | 95 ソース、hart 0 の MEIP/SEIP |
| NS16550A | `0x10000000` | バイト間隔の UART、PLIC ソース 10 |

`--dtb BASE:PATH` は RV64 のファームウェア入口で `a0` に hart ID、`a1` に DTB の物理アドレスを設定します。QEMU のリセット ROM が設定する引数に対応します。この手順では OpenSBI FW_JUMP を DDR 先頭から直接起動し、Linux の物理入口を `0x80200000` に固定します。DTB を `--load-binary` で置くだけの試行では `a1` が 0 のままで、OpenSBI の UART 出力まで進みませんでした。[QEMU の RV64 リセット ROM](https://github.com/qemu/qemu/blob/v9.2.0/hw/riscv/boot.c) [OpenSBI FW_JUMP の引数](https://github.com/riscv-software-src/opensbi/blob/v1.6/docs/firmware/fw_jump.md)

RISC-V クロスコンパイラー、DTC 一式、U-Boot の `dumpimage` が必要です。検証では下記の StarLight ビルド手順と同じ Bootlin GCC 10.3.0 を使いました。`$assets` は StarLight デモの公開ファイルを保存したディレクトリです。既存 FIT の Linux 5.10.6 と Buildroot initramfs を再利用し、起動ファームウェアと DTB は virt 用に用意します。

```sh
set -e
cargo build --release
work=$(mktemp -d)
fit="$assets/beaglev--image.fit-s_83025501-3c24fa8a3dd087ac05f6a17399ff2f44f673e1c4"
printf '%s  %s\n' '3c24fa8a3dd087ac05f6a17399ff2f44f673e1c4' "$fit" | sha1sum -c -
curl -fLsS https://codeload.github.com/riscv-software-src/opensbi/tar.gz/refs/tags/v1.6 \
  -o "$work/opensbi.tar.gz"
printf '%s  %s\n' 'd11702103f177a2914e94eec57ce5ed820296d874f6b6525c4482e55d71a3667' \
  "$work/opensbi.tar.gz" | sha256sum -c -
tar -xf "$work/opensbi.tar.gz" -C "$work"
make -C "$work/opensbi-1.6" -j8 CROSS_COMPILE="${CROSS_COMPILE:-riscv64-linux-gnu-}" \
  PLATFORM=generic FW_JUMP=y FW_JUMP_ADDR=0x80200000 FW_JUMP_FDT_ADDR=0x88000000
"${DUMPIMAGE:-dumpimage}" -T flat_dt -p 0 -o "$work/Image" "$fit"
"${DUMPIMAGE:-dumpimage}" -T flat_dt -p 1 -o "$work/rootfs.cpio" "$fit"
end=$(printf '%x' "$((0x84000000 + $(stat -c %s "$work/rootfs.cpio")))")
dtc -I dts -O dtb -o "$work/virt.dtb" platforms/rv64_virt.dts
fdtput -t x "$work/virt.dtb" /chosen linux,initrd-start 0 84000000
fdtput -t x "$work/virt.dtb" /chosen linux,initrd-end 0 "$end"
dtc -I dtb -O dtb -p 4096 -o "$work/virt-padded.dtb" "$work/virt.dtb"
target/release/penv run --platform platforms/rv64_virt.repl --raw --uart-stdin \
  --dtb "0x88000000:$work/virt-padded.dtb" --load-binary "0x80200000:$work/Image" \
  --load-binary "0x84000000:$work/rootfs.cpio" --max-steps 5000000000 \
  "$work/opensbi-1.6/build/platform/generic/firmware/fw_jump.bin"
```

Buildroot のプロンプトには、公開デモと同じ `root` / `starfive` を使います。initramfs の起動スクリプトは `eth0` を探してタイムアウトします。この virt 構成の残課題は VirtIO、PCIe、RTC、SiFive Test のリセットと電源断、QEMU の `a2` を使う FW_DYNAMIC、複数ハートの Linux 検証です。PLIC の閾値と MMIO 範囲には既存の Renode 向けモデルを使用します。

OpenSBI v1.6 が `Penv RV64 virt` と 10 MHz のタイマーを認識し、Linux 5.10.6 が `0x10000000` の `ttyS0` を有効にして `/init` を実行しました。UART のパスワード認証後、`id -u` が `0`、`uname -m` が `riscv64` を返しました。`cargo test` は全 49 件通過しました。CLI の回帰テストは 2 ハートの `a0` と 4 GiB 超の `a1`、DTB のマジックとアラインメントの検証を含みます。

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

#### Ethernet とマシンの復帰

```sh
python3 scripts/verify-starlight.py "$assets" --ports 30457 30458 \
  --switch-port 30456 --recovery --logs target/starlight-recovery
```

最初の ping に続いて、ゲスト A の `eth0` を down/up し、Linux の stmmac ドライバーに DMA を再初期化させます。DMA リセットは記述子・割り込みの状態とキューに残った受信フレームをクリアします。ドライバーを開き直した後、同じ UART シェルでコマンドを実行し、両方向の ping を確認します。[Linux 5.10 stmmac の DMA 初期化](https://github.com/torvalds/linux/blob/v5.10/drivers/net/ethernet/stmicro/stmmac/stmmac_main.c)

次に Linux の `reboot` が `Restarting system` を出力したところで、検証スクリプトがホスト側のマシンリセットを要求します。`--uart-stdin --uart-stdin-reset` では標準入力の `1d 72`（Ctrl-]、`r`）がリセット、`1d 1d` が UART へのリテラル `1d` です。リセット時は同じプロセス内で CPU、RAM、デバイスを作り直し、RESC のファームウェアと FIT を再ロードします。UART ストリームとスイッチを維持し、Ethernet は同じローカルポートに再接続します。`--max-steps` は全ブートの合計です。

公開 FIT のデバイスツリーには `gpio-restart` ノードがあります。同梱カーネルの埋め込み設定では `CONFIG_POWER_RESET_GPIO_RESTART` が無効で、`reboot` 単独の試行は `machine_restart`（PC `0xffffffe000201b94`）に留まりました。ボードの再起動にはホスト側のリセットが必要です。実機の GPIO やウォッチドッグによるリセット、RAM を保持するウォームリセットは対象外です。[Linux 5.10 RISC-V の再起動処理](https://github.com/torvalds/linux/blob/v5.10/arch/riscv/kernel/reset.c)

公開イメージで、DMA 再初期化後の UART コマンドと 56 バイトの両方向 ping を確認しました。マシンリセット後は A が OpenSBI から再起動して UART に再ログインし、`id -u` と `uname -m` が再び `0` と `riscv64` を返しました。A のネットワークを設定し直した後も、56 バイトと 3200 バイトの両方向 ping は各 10/10 応答でした。A の `eth0` IRQ は 0 から 104、起動を継続した B は 3 から 240 に増えました。`cargo test` は全 46 件通過しました。

#### Linux ゲストからの GPIO リセット

公開 FIT の `/soc/gpio-restart` は、`/soc/gpio@11910000` の GPIO10 をアクティブ Low で使います。同梱カーネルの `CONFIG_POWER_RESET_GPIO_RESTART` は無効で、`CONFIG_DEVMEM` と BusyBox の `devmem` は利用できます。従来の REPL の GPIO プレースホルダーに加え、StarLight ボードではこの GPIO の出力値と出力イネーブルを実際の JH7100 アドレスにモデル化します。起動時はハイインピーダンスで、出力を有効にして Low を駆動するとボードリセットが発生します。[StarFive JH7100 GPIO レジスター](https://lkml.rescloud.iu.edu/hypermail/linux/kernel/2106.3/06179.html)

```sh
python3 scripts/verify-starlight.py "$assets" --ports 30457 30458 \
  --switch-port 30456 --recovery --guest-reboot --logs target/starlight-guest-reboot
```

対応するゲスト側の操作は次のとおりです。StarLight の root シェルから実行します。

```sh
devmem 0x119100a0 32 1   # GPIO10 DOUT を High にする
devmem 0x119100a4 32 0   # GPIO10 DOEN を有効にする
sync
devmem 0x119100a0 32 0   # リセット線を Low にする
```

この操作はボードリセットを直接要求します。同梱カーネルの標準 `reboot` を GPIO に接続するには、GPIO 再起動ドライバーを有効にしたゲストカーネルが必要です。その構成でのシャットダウン処理と、GPIO10 以外の GPIO・割り込み・周辺機能へのピン多重化は残課題です。検証スクリプトの `--guest-reboot` はホスト UART リセットエスケープを無効にしたまま、GPIO を駆動して再ログインと通信復帰を確認します。

公開イメージで DOUT=`1`、DOEN=`0` を読み戻し、DOUT=`0` の書き込みで A の 2 回目の OpenSBI が始まりました。A は再認証し、`id -u` が `0`、`uname -m` が `riscv64` を返しました。再設定後は 56 バイトと 3200 バイトの両方向 ping が各 10/10 応答し、`eth0` IRQ は A が 0 → 100、稼働を継続した B が 3 → 245 に増えました。`cargo test` は全 48 件通過しました。

#### GPIO ドライバーを有効にした Linux の標準 reboot

ビルドには x86-64 Linux ホスト、C コンパイラーと開発ヘッダー、GNU Make、`bc`、`bison`、`flex`、OpenSSL 開発ヘッダー、`curl`、Python 3、Device Tree Compiler 一式、U-Boot の `mkimage` と `dumpimage` が必要です。上の手順で取得した公開ファームウェアを入力にします。スクリプトは公開カーネルのコミット `93604a0a3ecc4ef8efb9d877854f89df2c7208f7` と Bootlin の GCC 10.3.0 ツールチェーンを取得し、ハッシュを照合します。元の ELF に埋め込まれた設定を使い、`CONFIG_POWER_RESET_GPIO_RESTART=y` とリリース接尾辞 `-penv-gpio` を設定します。

```sh
out=target/starlight-linux-reboot-build
JOBS=12 scripts/build-starlight-linux-reboot.sh "$assets" "$out"
cargo build --release
python3 scripts/verify-starlight.py "$out/assets" --ports 30457 30458 \
  --switch-port 30456 --recovery --linux-reboot --logs target/starlight-linux-reboot
```

`MKIMAGE` と `DUMPIMAGE` でツールのパスを指定できます。Linux FIT は元の Buildroot initramfs と DTB を使用し、DTB の `gpio-restart` に `open-source` を追加します。この設定で GPIO10 はドライバーの probe 時に入力のままになり、再起動通知で初めて駆動されます。元の StarFive GPIO ドライバーは出力値を書き込む前に出力を有効にするため、この順序が必要です。ビルド済み資材は `$out/assets` に保存され、上流 RESC が参照するファイル名で読み込めます。[StarFive の GPIO 再起動ドライバー](https://github.com/starfive-tech/beagle_kernel_5.10/blob/93604a0a3ecc4ef8efb9d877854f89df2c7208f7/drivers/power/reset/gpio-restart.c) [StarFive の GPIO 出力制御](https://github.com/starfive-tech/beagle_kernel_5.10/blob/93604a0a3ecc4ef8efb9d877854f89df2c7208f7/drivers/gpio/gpio-starfive-vic.c)

別のビルドディレクトリで再構築し、カーネル `Image` と FIT のバイト列が一致することを確認しました。検証時のツールは Bootlin GCC 10.3.0、DTC 1.6.1、`mkimage` 2023.01 です。生成 FIT の SHA-256 は `d835ed80f222889adc54f5705c28b29d78ca62359581617a67bf14948279a8c1` です。

Linux `5.10.6-penv-gpio` で GPIO 再起動ドライバーへのバインドを確認しました。ゲスト A の標準 `reboot` からサービス停止、`Restarting system`、2 回目の OpenSBI、UART 再認証まで自動確認しました。再ログイン後の `id -u` は `0`、`uname -m` は `riscv64` を返し、56 バイトと 3200 バイトの ping は両方向で各 10/10 応答しました。ホスト側のリセットエスケープは無効のまま検証しました。`cargo test` は全 48 件通過しました。物理ボードでの検証とウォームリセット時の RAM 保持は残課題です。

### 上流 Renode の RV64 ケース対応

棚卸しの参照点は `../renode` の `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` です。上流の `platforms/cpus/riscv_virt.repl` は RV32 定義で、このリビジョンの Robot スイートには RV64 virt の Linux 起動ケースが見当たりません。RV64 virt の実ファームウェア検証は上記の OpenSBI FW_JUMP 手順で行い、RV64 CPU に共通する次のケースを Robot から Rust に移植しました。上流の `VirtioMMIOConsole.robot` は Cortex-A53/Zephyr を対象とします。[Renode riscv_virt.repl](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/riscv_virt.repl) [Renode VirtioMMIOConsole.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/peripherals/VirtioMMIOConsole.robot)

| 上流ケース | 移植先と確認内容 |
| --- | --- |
| `riscv-priv.robot` の `Should Increment MCYCLE RV64` / `Should Increment MINSTRET RV64` | `increments_mcycle` / `increments_minstret`。CSR 読み出し間の差分が 2 |
| 同 `Should Inhibit MCYCLE RV64` / `Should Inhibit MINSTRET RV64` | `inhibits_mcycle` / `inhibits_minstret`。抑止中の差分が 0 |
| 同 `Should Inhibit MCYCLE After Write Finishes RV64` / `Should Inhibit MINSTRET After Write Finishes RV64` | `inhibits_mcycle_after_write_finishes` / `inhibits_minstret_after_write_finishes`。抑止を書き替える命令を境界にした差分が 2 |
| 同 `Should Trap On SFENCE.VMA In User Mode RV64` | `traps_sfence_vma_in_user_mode`。例外の入口、原因 2、`mepc` |
| `riscv-amocas.robot` の `Amocas.{w,d,q} On RV64 Should Set Value At Single-Page Memory Location` | `amocas_{w,d,q}_changes_single_page_on_match`。旧値の返却、置換、アドレス・置換レジスター、書き込み幅 |
| 同 `Amocas.{w,d,q} On RV64 Shouldn't Set Value At Single-Page Memory Location` | `amocas_{w,d,q}_preserves_single_page_on_mismatch`。旧値の返却とメモリ保持 |

Rust 移植は `src/rv64/upstream_tests.rs` にあります。Monitor のレジスター設定と Step を Rust のレジスター初期化と実際の命令実行に置き換えました。`amocas.w` は旧値の下位 32 ビットに加えて、RV64 での符号拡張を検証します。ページ境界またぎ、MMIO、他の AMO、残りの特権・PMP・ISA 拡張ケースは未移植です。[Renode riscv-priv.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/riscv-priv.robot) [Renode riscv-amocas.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/riscv-amocas.robot)

| BeagleV の上流ケース | CLI での対応 |
| --- | --- |
| StarLight `Should Boot U-Boot` | 両ゲストの UART3 で OpenSBI v0.9、StarFive、U-Boot 2021.01、`dwmac.10020000` を順に照合 |
| StarLight `Should Provide Two Linux Machines With Ethernet Connection` | U-Boot 確認後にローカルスイッチを起動し、両ゲストにログインして上流と同じ MAC・IPv4・MTU 440 を設定 |
| StarLight `Should Ping` | B から A に 56 バイトを 10 回送り、全応答と損失 0% を確認 |
| StarLight `Should Ping Large Payload` | B から A に 3200 バイトを 10 回送り、全応答と損失 0% を確認 |
| StarLight `Should Boot U-Boot With External PMP` / `Should Provide Two Linux Machines With Ethernet Connection With External PMP` / `Should Ping With External PMP` | `Miscellaneous.RiscVExternalPMP` の配線が未実装のため保留 |
| Fire `Should Boot Buildroot From SD Card` | 部分検証。公開 HSS がキャッシュの WayEnable と `MPFS_Sysreg` を通過。L2 キャッシュの次のアトミックアクセスで停止。SD 起動と Buildroot ログインは未到達 |

CLI 移植は `verify-starlight.py --upstream-cases` です。上流と同じ公開 RESC・ファームウェアを使い、ケース名ごとに結果を出力します。Penv のゲストプロセスは U-Boot 確認とスイッチ接続の間も動作を続けます。Linux でのコマンド実行と `eth0` IRQ の確認も行います。[Renode BeagleV-StarLight.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/BeagleV-StarLight.robot) [Renode BeagleV-Fire.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/BeagleV-Fire.robot)

`monitor-tests.robot` にも Fire の REPL を使うケースがあります。配列プロパティ設定、`Select` の整形、スカラー型エラーを検証する Monitor ケースなので、UART 起動や RV64 CPU の優先ケースとは別に保留しました。[Renode monitor-tests.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/monitor-tests.robot)

```sh
cargo test rv64::upstream_tests
cargo test
cargo build --release
python3 scripts/verify-starlight.py "$assets" --ports 30457 30458 \
  --switch-port 30456 --upstream-cases --logs target/rv64-upstream-tests/starlight
```

移植した Rust ケース 13 件を含む `cargo test` 全 62 件と、StarLight の CLI ケース 4 件が通過しました。56 バイトと 3200 バイトは各 10/10 応答で、`eth0` IRQ は A が 3 → 61、B が 3 → 54 でした。RV64 virt の OpenSBI v1.6 → Linux 5.10.6 も再実行し、UART ログイン後の `id -u` が `0`、`uname -m` が `riscv64` を返しました。

### BeagleV-Fire の SD 起動の再現

参照リビジョンの SD 起動テストは BeagleV-Fire（PolarFire SoC）にあります。BeagleV-StarLight の公開デモは RAM の `0xa0000000` に読み込んだ FIT を起動します。Fire の Robot ケースは OpenSBI 1.2、U-Boot 2023.07、Linux の root ログイン、`uname -m` の `riscv64`、`uname -n` の `buildroot` を検証します。[Fire のテスト](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/BeagleV-Fire.robot) [Fire の RESC](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/scripts/single-node/beaglev-fire.resc) [StarLight の RESC](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/scripts/single-node/beaglev_starlight.resc)

```sh
cargo test
cargo build --release
fire_assets=$(mktemp -d)
fire_resc=../renode/scripts/single-node/beaglev-fire.resc
rg -o 'https://[^[:space:]]+' "$fire_resc" | while IFS= read -r url; do
  name=${url##*/}
  curl -fLsS --retry 2 "$url" -o "$fire_assets/$name" &&
    printf '%s  %s\n' "${name##*-}" "$fire_assets/$name" | sha1sum -c - || exit 1
done

# 上流のスクリプトは 12 行目の SdCardFromFile で停止する。
target/release/penv run --script "$fire_resc" --platform-root ../renode \
  --asset-dir "$fire_assets" --max-steps 100000

# 上流 REPL と HSS ELF を直接指定し、e51 の到達 PC を確認する。
for steps in 3763661 3763760 3763761; do
  target/release/penv run --platform ../renode/platforms/boards/beaglev-fire.repl \
    --platform-root ../renode --uart-device mmuart0 --max-steps "$steps" --stats \
    "$fire_assets"/hss-envm-wrapper.elf-s_*
done
```

変更前の `11b9d76` では同じ 3,763,660 ステップで `pc=0x20220350` に入りました。HSS は e51 の `pc=0x20231a74` で `amoswap.d` によるストアアクセス例外（原因 7、`mtval=0x02010008`）を起こしました。上流 REPL の `CacheConfig_WayEnable` と同じ初期値 0 の保持レジスターを追加し、HSS が使うアトミック交換をそのレジスターに実装しました。CLI テスト `polarfire_hss_can_swap_the_cache_way_enable_register` が初期値、64 ビットの交換前の値、ロードとストアの読み戻しを確認します。[PolarFire SoC の REPL](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/polarfire-soc.repl)

コミット `9807d54` では、3,763,661 ステップ目の `MPFS_Sysreg` 書き込みで早期トラップハンドラー `0x20220350` に入りました。上流の SYSREG は 4 KiB の 32 ビット MMIO で、HSS が書き込むオフセット `0x174` は未定義です。今回の実装ではこの書き込みを受け付けて破棄し、未定義オフセットは 0 を返します。上流のリセット値とクロック・ソフトリセットレジスターの書き込み可能ビットを再現しました。各周辺機器へのクロック制御とリセットの配線は残課題です。[Renode MPFS_Sysreg](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Miscellaneous/MPFS_Sysreg.cs)

| 実行ステップ数 | 変更後の e51 の PC |
| --- | --- |
| 3,763,661 | `0x20231a80`（SYSREG 書き込みの次の命令） |
| 3,763,760 | `0x20231ad2`（`0x02010800` に対する `amoswap.d`） |
| 3,763,761 | `0x20220350`（早期トラップハンドラー） |

次の例外は L2 キャッシュ設定へのアトミックアクセスです。対象アドレスは上流 REPL の `SilenceRange` に含まれます。現在の Penv はこの範囲の通常ロード／ストアに対応し、`amoswap.d` はそこでストアアクセス例外（原因 7、`mtval=0x02010800`）になります。この段階の Fire の UART 出力は空です。[PolarFire SoC の REPL](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/polarfire-soc.repl)

上流の `SdCardFromFile $image mmc 0x100000000` は、ファイルを裏付けにした公称容量 4 GiB の SD カードを `mmc` に接続します。永続化は既定で有効です。`mmc` は `0x20008000` にマップされる MPFS SD コントローラーで、PLIC の IRQ 88 と WakeupIRQ 89 に接続されます。Penv の RESC はこのコマンドで停止します。SD カードの接続、コントローラーの SD コマンド・データ転送・割り込み、上流 RESC が使う GPIO ボタンと起動用 Monitor コマンドは残課題です。Fire の OpenSBI、U-Boot、Linux は未到達です。[Renode SDCardExtensions](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SD/SDCardExtensions.cs) [Renode MPFS_SDController](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SD/MPFS_SDController.cs)

今回の `cargo test` は全 64 件が通過しました。CLI テスト `polarfire_sysreg_handles_hss_access_and_register_defaults` は HSS と同じオフセットへの書き込みと、PLL 状態およびクロック・ソフトリセットレジスターの読み戻しを確認します。

前回は上記の StarLight 公開素材を使った `python3 scripts/verify-starlight.py "$assets"` でも、UART から root にログインし、`id -u` が `0`、`uname -m` が `riscv64` を返しました。

## Cortex-M0 / Cortex-M0+

REPL の `CPU.CortexM` で `cpuType: "cortex-m0"` または `cpuType: "cortex-m0+"` を指定すると、限定的な Thumb 実行系を使用できます。`--raw` はイメージを `vectorTableOffset`（既定値 0）に読み込み、ベクターテーブルから初期 SP と PC を設定します。整数演算、分岐、即値オフセットでのロード・ストア、LiteX UART への出力を確認済みです。`BKPT` で実行を終了します。32 ビットの BL と、PUSH・POP、SP 相対のロード・ストアにも対応します。ARM EABI5 の静的 ELF32 は物理ロードアドレスを使用します。明示的なベクターアドレスがない場合は、ロードされる割り当て済みセクションの最小物理アドレスを使い、該当セクションがなければロードセグメントの先頭を使います。リセット時の SP と PC はそのベクターテーブルから取得します。起動後は VTOR への書き込みで、例外用のベクターテーブルをマップ済みの 128 バイト境界に移動できます。SVC はベクターテーブル経由でハンドラーを呼び出し、MSP を使って元の実行状態を復元できます。SysTick は実行ステップをクロックとして動作し、CPSID/CPSIE で割り込みをマスクできます。NVIC は 32 本の外部 IRQ の有効化、保留、優先度を管理し、REPL で LiteX UART を接続できます。高優先度の外部 IRQ または SysTick は実行中のハンドラーに割り込み、MSP で元のハンドラーに復帰できます。LDM/STM、DSB/DMB/ISB、および MSP、PRIMASK、PSR の MRS/MSR にも対応します。CONTROL.SPSEL でスレッドのスタックを切り替え、PSP を使った例外フレームから復帰できます。PendSV は ICSR から保留・解除でき、SHPR3 の優先度に従って配送されます。SVC のエスカレーション、CONTROL.nPRIV、その他の 32 ビット Thumb 命令は未対応です。

Cortex-M の REPL で `UART.STM32F7_USART` を複数配置し、各 USART を NVIC に直接接続するか、STM32F4 EXTI や CombinedInput を経由して接続できます。受信は USART と受信器を有効にした後に届けてください。標準入力用のファイルは、STM32 USART だけを配置した場合は最初に宣言した USART へ送られます。送信は即時で、DMA 配線は未対応です。

## 現在の実装範囲

RV32I 基本整数命令、M 拡張、Zifencei を実装しています。REPL で `rv32ima` または `rv32ima_zicsr_zifencei` を指定すると、共有 RAM 上の A 拡張命令を使用できます。`rv32imc` と `rv32imac` では RV32C の整数圧縮命令に対応します。`cycle`、`instret`、`time` と各上位 32 ビットを読み取れます。CLINT がある場合の `time` は `mtime` に従います。Zicsr の命令と、マシンモードの CSR の一部、および `ECALL`・`EBREAK` からの `MRET` を実装しています。`CPU.RiscV32` ではユーザーモードへの移行とマシンモードへのトラップ、16 エントリーの PMP に対応しています。スーパーバイザーモードでは同期例外の委譲と `SRET`、32 ビット物理アドレス範囲の Sv32 ページ変換に対応しています。変換はハートごとに 64 エントリーをキャッシュし、SFENCE.VMA でそのハートのキャッシュを無効化します。マシンモードから設定したスーパーバイザー割り込みを委譲できます。3 コンテキスト以上の PLIC、その他の ISA 拡張、REPL のその他の構文とデバイス、起動コマンド以外の RESC 構文、ネットワーク、複数ノードは未対応です。標準構成の UART は単一の送信レジスターを提供します。

RV32I の仕様は [RISC-V ISA マニュアル](https://docs.riscv.org/reference/isa/unpriv/rv32.html) を参照してください。
