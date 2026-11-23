# penv

`penv` は、Renode のファームウェア実行機能を Rust で段階的に実装する CLI エミュレーターです。現在はベアメタル RV32IM を実行できます。フレーム待機用の PNG 読み込みには Rust の `png` クレートを使用します。

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

上流の `platforms/cpus/litex_vexriscv_smp.repl` では、4 ハートから LiteX UART にハート ID を出力できることを確認しました。LiteX UART は即時送信、ポーリング受信、VexRiscv のマシン外部 IRQ への接続に対応します。送信 FIFO の遅延は未対応です。LiteX の 8 ビットおよび 32 ビット CSR タイマーは、既定の 100 MIPS に相当する実行ステップと REPL の周波数から進行します。RAM、LiteX UART、LiteX タイマーの複数の sysbus アドレスは同じ状態を共有します。

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

LiteX のスイッチとボタンは `--litex-button-event STEP:NAME:ACTION` で実行中に操作できます。例えば `--litex-button-event 71000000:cas.switch0:Toggle` はステップ 71,000,000 で Arty のスイッチ 0 を反転します。`Press`、`Release`、`Toggle` を指定でき、同じステップのボタン操作は指定順に適用されます。指定した操作はそのステップのゲスト命令より先に反映されます。

Murax の UART エコー、LiteX Ibex BIOS のシェル、LiteX VexRiscv Zephyr のシェル、Arty の LED・スイッチ・ボタン・PHY ステータス、LiteX 単一ハート Linux の起動・ログイン・CPU 表示、LiteX SMP Linux の起動・ログイン・4 ハートの表示、Nexys Video の SD 起動・フレームバッファ、Zephyr I²S の PCM バイト一致、Minerva の UART ブートをまとめて検証できます。検証スクリプトは上流の RESC をそのまま使い、ファームウェアのサイズと SHA-1 を照合します。取得済みファイルは出力先の `assets/` に置くと再利用されます。Minerva の UART ブート確認には Python 3.10 以降を使います。

```sh
bash scripts/validate-riscv-upstream.sh target/release/penv ../renode /tmp/penv-upstream
```

検証結果には上流のコミット ID、UART 出力、標準エラー出力、各実行の終了コードが含まれます。LiteX Ibex は上流の `litex_ibex.resc` と `litex_ibex.repl` を使用し、BIOS の CRC 検証、シリアルブートの中断、`help` の実行を確認します。上流の `litex_vexriscv_zephyr.resc` では UART シェルの `version` を確認します。LiteX Ethernet はネットワーク未接続時のレジスターとパケットバッファーを実装しています。`litex_vexriscv_linux.resc` では 75 MHz の CPU タイマーで Linux 5.0.14 が `/init` を実行するまでを検証します。REPL に接続された PHY のレジスターは MDIO で読み取れます。外部ネットワークとのパケット送受信、GPIO の外部配線、SPI スレーブ接続は未対応です。

### RV32 上流検証記録

上流 `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` を使用しました。変更前の `f9e5acc` では Arty の `H2U` が起動してスイッチ初期値 `0` を読めますが、上記の `--litex-button-event` は `unknown option` で終了します。変更後の検証スクリプトは同じ上流 RESC と ELF を使い、UART の応答を順に比較します。

| 検証 | 結果 |
| --- | --- |
| `cargo test` | ユニット 45 件、CLI 7 件が成功 |
| Arty の `debug cas leds` | 上流の LED テストと同じマスク `0 → 1 → 3 → 7 → 15 → 0` で 4 灯を毎回検証 |
| Arty の `debug cas switches` | 初期値 `0`、反転後 `1` |
| Arty の `debug cas buttons read` | 押下後 `1 1`、解放後 `0 1`、ラッチクリア後 `0 0` |
| Arty の PHY | 起動時は `10Mbps / link: down`。実行中の変更後に `mdio_status` が `100Mbps / link: up` を返す |
| LiteX RV32 Linux | 単一ハートと SMP で Buildroot にログインし、`/proc/cpuinfo` でそれぞれ 1 ハートと 4 ハートを確認 |
| Minerva UART | 上流 BIOS の SFL で RV32 検証ペイロードを `0x40000000` に転送し、ジャンプ後にペイロードが UART へ `~` を出力 |
| Zephyr I²S | 入出力 793,344 バイトが `cmp` で一致。SHA-256 は `e8dca0ec23b1113e7d47e2fcd16ef64de6d8eec9cdd868dc86527d53d4bba116` |

Minerva の検証では、BIOS が参照するタイマーアドレスを `--litex-timer-alias 0xe0002000` で追加し、`--litex-uart-legacy-console` と `--litex-uart-stdio` で SFL の応答を送受信します。

### 対象 LiteX ケースの一括検証

上流 Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の RESC・REPL と公開イメージを使用しました。上記の `scripts/validate-riscv-upstream.sh` を一度実行すると、次のケースを照合し、ステップ上限を使うケースと成功終了を期待するケースの終了コードも区別します。

| 対象 | 上流の出典 | 照合内容 |
| --- | --- | --- |
| 単一ハート Linux | `litex_vexriscv_linux.resc` | 75 MHz のカーネル時刻源、`/init`、`hvc0` ログイン、`/proc/cpuinfo` に CPU 0 のみ |
| SMP Linux | `LiteX_VexRiscv_SMP.robot` の OpenSBI と Linux 起動 | OpenSBI と Linux の 4 ハート、Buildroot ログイン、CPU 0～3 |
| Minerva UART | `litex_minerva.resc` の公開 BIOS | SFL で検証ペイロードを転送し、ジャンプ後にペイロード自身の `~` を UART で受信 |
| Arty | `Arty_LiteX_VexRiscv.robot` | 4 灯の LED、全スイッチ・ボタンの遷移、起動時リンクダウンと実行中変更後のリンクアップ |
| I²S | `LiteX_VexRiscv_I2S.robot` の `Should Echo Audio` | `RunFor "3.2"`、送信器の `Dispose`、793,344 バイトの PCM 完全一致 |
| Nexys Video | `Nexys_Video_LiteX_VexRiscv.robot` の全画面・領域テスト | 公開 PNG を無変換で照合。全画面と左上 75×80 は別起動で同じ UART 入力を再生し、それぞれ終了コード 0 を確認 |
| Nexys4DDR SPI SD | `Nexys4DDR_LiteX_VexRiscv.robot` の起動・README 列挙 | 公開 BIOS と FAT16 イメージで `Q`、`spisdcardboot`、`README  .TXT` の列挙 |

単一ハート Linux はログインと CPU 表示まで通過します。ゲストログに出る tmpfs マウントとネットワーク初期化の失敗、Robot Framework API の直接互換、同一起動での連続フレーム待機、SPI SD の書き込みとマルチブロック転送は残課題です。

### Arty の LED 状態検証

`--litex-led-assert STEP:NAME:on|off` を繰り返して、REPL で配線された LED の状態を指定ステップのゲスト命令より前に検証します。名前は `sysbus.cas.led0` または `cas.led0` のように指定します。状態が異なる場合や実行が検証ステップに届かない場合は、終了コード 1 と診断を返します。

変更前の `abc723e` はこのオプションを `unknown option` として拒否しました。上記の検証スクリプトは上流 Arty の RESC と実 ELF を使い、`debug cas leds` を UART から送り、4 灯の初期状態と各操作後の状態を計 24 件検証します。次のコマンドでは LED 0 を点灯させ、意図的に消灯を期待して失敗を確認できます。

```sh
artifacts=/tmp/penv-upstream
target/release/penv run --script ../renode/scripts/single-node/arty_litex_vexriscv.resc \
  --platform-root ../renode --asset-dir "$artifacts/assets" --max-steps 73000000 \
  --uart-event "70000000:$artifacts/arty-leds-1.input" \
  --litex-led-assert 72000000:sysbus.cas.led0:off
```

### Arty の PHY 設定変更

`--litex-phy-event STEP:NAME:VALUE` は、REPL に宣言された PHY の `VendorSpecific1`（MDIO レジスター 17）へ 16 ビット値を書き込みます。ステップ 0 は最初のゲスト命令より前に適用されます。名前は `eth.phy` または `sysbus.eth.phy` で指定でき、値は 10 進数または `0x` 付きの 16 進数です。

変更前の `abeac21` では下記のオプションが `unknown option` でした。上記の回帰スクリプトで取得した同じ上流 ELF を使って単独でも再現できます。

```sh
artifacts=/tmp/penv-upstream
printf 'mdio_status\r' > "$artifacts/arty-mdio.input"
target/release/penv run --script ../renode/scripts/single-node/arty_litex_vexriscv.resc \
  --platform-root ../renode --asset-dir "$artifacts/assets" --max-steps 100000000 \
  --litex-phy-event 0:eth.phy:0x0 \
  --litex-phy-event 97000000:sysbus.eth.phy:0x4400 \
  --uart-event "98000000:$artifacts/arty-mdio.input"
```

実ファームウェアの起動ログに `MDIO mode: 10Mbps / link: down`、コマンド応答に `MDIO mode: 100Mbps / link: up` がこの順で出力されます。ステップ上限で終了コード 2 になります。他の PHY プロパティの実行中設定と Renode の Robot Framework テスター API 互換性は残課題です。性能計測と最適化は後工程で行います。

### Nexys Video のフレームバッファ検証

上流の `litex_nexys_video_vexriscv_linux.resc` と `Nexys_Video_LiteX_VexRiscv.robot` を使用しました。変更前の `ae51015` は SD カードから起動して最終 PPM を出力できましたが、`--framebuffer-event 675000000:/tmp/logo.ppm` は `unknown option` でした。同じ起動の途中でロゴ画面と MOTD 画面を保存できるようにしました。

`--framebuffer-event STEP:PATH` は繰り返し指定でき、各ステップのゲスト命令より前に現在の LiteX フレームバッファを P6 RGB PPM へ保存します。DMA が無効な場合や、実行が指定ステップに届かなかった場合は終了コード 1 と診断を返します。

```sh
cargo test
cargo build --release
bash scripts/validate-riscv-upstream.sh target/release/penv ../renode /tmp/penv-upstream
```

回帰スクリプトは上流の BIOS ELF と SD イメージのサイズと SHA-1 を確認します。BIOS の自動起動を `Q` で中断し、`sdcardboot` を実行します。ステップ 675,000,000 でロゴを保存し、Buildroot にログインしてから `cat /etc/motd > /dev/tty0` を実行し、公開 PNG と全画面が一致した時点で MOTD 画面を保存します。領域待機は同じ UART 入力を別の起動で再生します。上流の参照 PNG を RGB に変換し、`penguin_logo.png` は左上 75×80 の ROI、`penguin_with_litex_logo.png` は 800×600 全体を SHA-256 で照合します。ロゴ撮影時はコンソールのカーソルが ROI 外の 16 ピクセルに影響するため、ロゴの照合範囲は上流テストと同じ ROI にしました。結果は `nexys-video.frames.log` と `nexys-video.uart.log` に出力されます。

### Nexys Video のフレーム一致待機

`--wait-for-frame STEP:PATH` は指定ステップから 1,000,000 ステップごとに有効なフレームの寸法と全 RGB ピクセルを比較します。一致したステップのゲスト命令を実行する前に停止し、終了コード 0 を返します。実行上限やゲストの終了までに一致を観測できなければ終了コード 1 です。参照形式は PNG または `--framebuffer-out` が出力する P6 RGB PPM（マジック、幅と高さ、最大値 255 がそれぞれ独立した行）です。

変更前の `49e1a6e` ではこのオプションが `unknown option` でした。上記の回帰コマンドを実行した後、上流 PNG 由来の SHA-256 と照合済みの PPM を参照にして次の手順で再現できます。

```sh
artifacts=/tmp/penv-upstream
penv=target/release/penv
"$penv" run --script ../renode/scripts/single-node/litex_nexys_video_vexriscv_linux.resc \
  --platform-root ../renode --asset-dir "$artifacts/assets" --max-steps 730000000 \
  --uart-event "35000000:$artifacts/nexys-video-abort.input" \
  --uart-event "40000000:$artifacts/nexys-video-sd.input" \
  --uart-event "680000000:$artifacts/nexys-video-login.input" \
  --uart-event "690000000:$artifacts/nexys-video-prompt.input" \
  --uart-event "700000000:$artifacts/nexys-video-motd.input" \
  --framebuffer-event "700000000:$artifacts/pre-motd.ppm" \
  --wait-for-frame "700000000:$artifacts/nexys-video-motd.ppm" \
  --framebuffer-out "$artifacts/matched.ppm"
cmp "$artifacts/nexys-video-motd.ppm" "$artifacts/matched.ppm"
```

上流の `penguin_with_litex_logo.png` から独立に変換した参照で、開始ステップでは不一致、708,000,000 ステップで一致を確認しました。一致時に保存した RGB PPM は参照とバイト単位で一致しました。上限を 2,000,000 ステップとして BIOS 起動直後から待った場合は、未一致の診断と終了コード 1 を確認しました。Robot Framework からの呼び出しは残課題です。

### Nexys Video の領域一致待機

`--wait-for-frame-roi STEP:X:Y:WIDTH:HEIGHT:PATH` は、参照画像とゲストのフレーム内の同じ矩形だけを比較します。座標の原点は左上です。矩形は空でなく、参照画像に収まる必要があります。ゲスト側で矩形が収まるフレームが表示されるまで待機できます。ポーリング間隔と終了コードは全画面待機と共通です。

変更前の `e85f1d9` では `--wait-for-frame-roi` は `unknown option` でした。回帰スクリプトで照合済みのロゴ PPM を使い、上流 `WaitForFrameROI` と同じ左上 75×80 の確認と、領域内の 1 チャンネルを変えた不一致を再現できます。

```sh
artifacts=/tmp/penv-upstream
python3 - "$artifacts/nexys-video-logo.ppm" "$artifacts/roi-changed.ppm" <<'PYTHON'
from pathlib import Path
import sys

header = b"P6\n800 600\n255\n"
data = bytearray(Path(sys.argv[1]).read_bytes())
assert data.startswith(header)
data[len(header)] ^= 255
Path(sys.argv[2]).write_bytes(data)
PYTHON

wait_roi() {
  target/release/penv run --script ../renode/scripts/single-node/litex_nexys_video_vexriscv_linux.resc \
    --platform-root ../renode --asset-dir "$artifacts/assets" --max-steps 730000000 \
    --uart-event "35000000:$artifacts/nexys-video-abort.input" \
    --uart-event "40000000:$artifacts/nexys-video-sd.input" \
    --uart-event "680000000:$artifacts/nexys-video-login.input" \
    --uart-event "690000000:$artifacts/nexys-video-prompt.input" \
    --uart-event "700000000:$artifacts/nexys-video-motd.input" \
    --wait-for-frame-roi "710000000:0:0:75:80:$1"
}
wait_roi "$artifacts/nexys-video-logo.ppm"
status=0
wait_roi "$artifacts/roi-changed.ppm" || status=$?
test "$status" -eq 1
```

実ファームウェアでは、上流の `penguin_logo.png` から変換した参照とステップ 710,000,000 で領域が一致しました。領域外には MOTD 描画による差がありました。領域内の 1 チャンネルを反転した参照は、上限 730,000,000 まで一致せず終了コード 1 でした。全画面待機もステップ 708,000,000 で一致しました。Robot Framework の API 互換性は残課題です。

### Nexys Video の PNG 参照

変更前の `1d15a71` では、参照 PNG を渡すと P6 PPM を要求して終了しました。現在はファイルのシグネチャで PNG を識別し、`--wait-for-frame` と `--wait-for-frame-roi` の両方で直接使えます。静止 PNG のサンプルを 8 ビット RGB に正規化します。比較には RGB 値を使い、アルファ値や色管理メタデータは適用しません。展開バッファと RGB データはそれぞれ最大 64 MiB です。

上記の回帰スクリプトで BIOS、SD イメージ、UART 入力を準備した後、上流 Robot シナリオの参照 PNG を取得してそのまま渡します。

```sh
artifacts=/tmp/penv-upstream
robot=../renode/tests/platforms/LiteX/Nexys_Video_LiteX_VexRiscv.robot
while IFS= read -r url; do
  name=${url##*/}
  curl --fail --location --silent --show-error "$url" -o "$artifacts/assets/$name" || exit 1
  printf '%s  %s\n' "${name##*-}" "$artifacts/assets/$name" | sha1sum --check || exit 1
done < <(rg -o 'https://[^[:space:]]+' "$robot")

run_nexys() {
  target/release/penv run --script ../renode/scripts/single-node/litex_nexys_video_vexriscv_linux.resc \
    --platform-root ../renode --asset-dir "$artifacts/assets" --max-steps 730000000 \
    --uart-event "35000000:$artifacts/nexys-video-abort.input" \
    --uart-event "40000000:$artifacts/nexys-video-sd.input" \
    --uart-event "680000000:$artifacts/nexys-video-login.input" \
    --uart-event "690000000:$artifacts/nexys-video-prompt.input" \
    --uart-event "700000000:$artifacts/nexys-video-motd.input" "$@"
}
full="$artifacts/assets/penguin_with_litex_logo.png-s_13404-8664342616adc3edda335db849dd1f5cd1805596"
roi="$artifacts/assets/penguin_logo.png-s_14782-2e0fc81dbcd56b3ed9c86790fe8d390a935395c5"
run_nexys --wait-for-frame "700000000:$full" \
  --wait-for-frame-roi "710000000:0:0:75:80:$roi"
```

一度の起動で指定順に画像を待ち、最後の一致で終了します。上流ファームウェアで、全画面はステップ 708,000,000、左上 75×80 の領域はステップ 710,000,000 で一致し、両方とも終了コード 0 でした。参照 URL の CLI からの取得と Robot Framework のテスター API 互換性は残課題です。

## RV32 riscv_virt の UART とタイマー

上流 `platforms/cpus/riscv_virt.repl` の RAM・NS16550・CLINT を使い、Clang と LLD でビルドした RV32IMAC ELF を実行します。検証 ELF は UART に起動マーカーを出力し、`mtimecmp = mtime + 64` を設定します。割り込みハンドラで機械タイマーの `mcause` と `mcycle` の差を確認し、割り込みを解除します。`mret` で戻った後に UART に成功マーカーを出力します。

```sh
cargo test --locked
cargo build --locked --release
out=$(mktemp -d)
clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
  -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld \
  scripts/firmware/rv32-virt.S -o "$out/rv32-virt.elf"
status=0
target/release/penv run --platform ../renode/platforms/cpus/riscv_virt.repl \
  --platform-root ../renode --max-steps 10000 "$out/rv32-virt.elf" \
  > "$out/uart.log" 2> "$out/stderr.log" || status=$?
test "$status" -eq 2 &&
diff -u <(printf '%s\n' 'rv32-virt: NS16550 UART ready' \
  'rv32-virt: 10 MHz CLINT machine timer IRQ and mret OK') "$out/uart.log"
```

変更前の `d3ba7be` では UART 出力と割り込み進入はできましたが、CLINT が REPL の `frequency: 10000000` を使わず、タイマーが期待より早く発火しました。修正後は 100 MHz の実行ステップ基準で 10 ステップにつき `mtime` が 1 進みます。テスト ELF はステップ上限まで待機ループに留まるため、終了コード 2 が期待値です。

既存の LiteX VexRiscv 専用 CLINT 配線と `--clint` は従来の 1 ステップ 1 ティックを使います。LiteX の回帰は `scripts/validate-riscv-upstream.sh` で確認します。virt の PLIC 経由 UART 受信割り込みは下記の ELF で検証できます。virt のマルチハート起動は未検証です。

## 上流 RV32 テストの対応表

上流 Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` を参照しました。下表の Robot ファイルは上流の `tests/` からの相対パスです。

| 対象 | 上流の出典とケース | penv のテストと検証範囲 |
| --- | --- | --- |
| RV32 CPU | `unit-tests/riscv-mret.robot` の `User Level Exists 32Bits` と `User Level Does Not Exists 32Bits` | `mret_restores_pc_and_resets_mpp_to_lowest_supported_privilege`。読み取り専用 `cycle` CSR への書き込みで例外を起こし、ハンドラ進入時の PC と MPP、`mret` 復帰後の PC と MPP を両構成で検査 |
| RV32 アーキテクチャ | `unit-tests/riscv-arch-tests.robot` の `Should Run Arch Tests 32` と `ArrayMemoryRiscVTestWatcher.cs` | `--tohost` と `--tohost-act` で合否を取得。公開 `riscv-tests` の I/M/A/C 122 件と、M/S 特権テスト 22 件中 21 件が通過。公開 ACT I/M の 47 ELF は F 命令を含む共通初期化で停止 |
| RV32 例外 | `unit-tests/riscv-ebreak.robot` の `Should Generate Ebreak` | 上流 ELF を同じ UART 配置の RV32IM サブセット REPL で実行し、ecall／ebreak を各 2 回処理して `finished test` を出力 |
| RV32 カウンタ | `unit-tests/riscv-priv.robot` の MCYCLE / MINSTRET の増加・停止・書き込み確定境界（RV32 各 3 ケース）と `Should Fail On Counter Access`（RV64） | `scripts/firmware/rv32-counters.S` で 6 ケースと、RV32 U モードのアクセス拒否・再許可を実 ELF 化 |
| LiteX Linux 入出力 | `platforms/LiteX/LiteX_Linux_VexRiscv.robot` の起動・LED・ボタン・SPI・I²C | Robot と同じ REPL と起動ファイルで、GPIO、SPI 受信列、I²C 列挙、温度値の変化を検証 |
| Nexys4DDR | `platforms/LiteX/Nexys4DDR_LiteX_VexRiscv.robot` の `Should Boot` / `Should List Readme File` | 上流 BIOS と FAT16 で、ブート中断、`spisdcardboot`、ディレクトリ列挙を検証 |
| LiteX TFTP | `network-server/NetworkServer.robot` の `Download File Over TFTP` | 同じ BIOS と MicroPython を上流の TFTP デモ RESC で使い、ブート中断後の `netboot`、転送バイト数、UART の `2 + 3` → `5` を検証 |
| LiteX SD | `platforms/LiteX/LiteX_VexRiscv_Linux_SDCard.robot` の `Should Load RootFS From SD Card` / `Should Mount Filesystem From SD Card` | 上流 RESC の ext4 起動・ログイン・`linuxrc` と、Robot 由来の FAT16 構成で読み出し・書き込み・再マウントを検証 |
| LiteX Tock | `platforms/LiteX/LiteX_Tock.robot` の `Should Run Tock` | 上流 RESC と ELF で起動行と `D1 → D2` の 3 組を UART から照合 |
| CLINT | `unit-tests/timer-reconfiguration.robot` の `Should Tick At Exact Time` | `clint_traps_at_the_exact_reprogrammed_compare_tick`。ゲストが `mtimecmp` の上下ワードを書き換え、5 ティックでは未発火、6 ティック目で機械タイマー例外に入る。virt の 10 MHz に合わせて 59 ステップと 60 ステップに適応 |
| RV32 TVM | `unit-tests/riscv-priv.robot` の SATP 読み書き・`SFENCE.VMA` に対する `When TVM Is One/Zero` 各ケース（上流は RV64） | RV32 版の Rust テスト `privilege_controls_gate_sret_and_address_translation_operations` と `scripts/firmware/rv32-tvm.S`。実 ELF は Sv32 有効時の M→S 遷移、S→M トラップ、TVM 解除後の再実行を確認 |
| RV32 Sv32 / TLB | `unit-tests/riscv-priv.robot` の `Should Not Trap On SFENCE.VMA When TVM Is Zero`（上流は RV64）と `unit-tests/riscv-mret.robot` の `Should Invalidate TLB After Mode Change` | 前者を RV32 S モードへ適応し、`rv32-tlb.S` でページテーブル更新後の命令・データ変換を検証。後者の上流 ELF は Bare/PMP の別ケースとして検証 |
| virt UART | `platforms/cpus/riscv_virt.repl`（上流 REPL。Robot ケースの移植元ではなく配線の出典） | `virt_ns16550_receive_interrupt_routes_through_plic` と `TEST_UART_IRQ` ELF。外部割り込み、PLIC ソース 3 の claim/complete、受信バイトと割り込み解除を検査 |
| LiteX UART | `platforms/LiteX/LiteX_Ibex.robot` の `Should Boot` と `Should Display Help` | `litex_receives_boot_abort_and_shell_command_in_separate_events` は `Q` と `help\r` を別々に投入し、バイト順と RX 割り込みの解除を検査。BIOS のプロンプトと help 応答は実 ELF を使う既存回帰で検査 |
| LiteX の既存回帰 | `LiteX_VexRiscv_SMP.robot` の OpenSBI・Linux、`LiteX_VexRiscv_I2S.robot` のオーディオ、`Arty_LiteX_VexRiscv.robot` と `Nexys_Video_LiteX_VexRiscv.robot` の入出力 | `scripts/validate-riscv-upstream.sh` に対応する検証あり。Arty のスイッチ・ボタンは各 4 ピンで上流の値の遷移を検証 |

virt の機械タイマーと NS16550 を同じ ELF で再現するには、上記の手順に `-DTEST_UART_IRQ` と `--uart-input` を追加します。

```sh
cargo test --locked
cargo build --locked --release
out=$(mktemp -d)
clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
  -nostdlib -fuse-ld=lld -DTEST_UART_IRQ -Wl,-T,scripts/firmware/rv32-virt.ld \
  scripts/firmware/rv32-virt.S -o "$out/rv32-virt.elf"
printf Q > "$out/uart.input"
status=0
target/release/penv run --platform ../renode/platforms/cpus/riscv_virt.repl \
  --platform-root ../renode --max-steps 10000 --uart-input "$out/uart.input" \
  "$out/rv32-virt.elf" > "$out/virt.uart" || status=$?
test "$status" -eq 2 &&
diff -u <(printf '%s\n' 'rv32-virt: NS16550 UART ready' \
  'rv32-virt: 10 MHz CLINT machine timer IRQ and mret OK' Q \
  'rv32-virt: NS16550 PLIC RX IRQ and mret OK') "$out/virt.uart"
bash scripts/validate-riscv-upstream.sh target/release/penv ../renode "$out/litex"
```

`cargo test --locked` は 60 件、virt の実 ELF と LiteX の実ファームウェア回帰はすべて成功しました。残課題は `riscv-arch-tests.robot` の RV32 アーキテクチャスイート、CLINT の翻訳ブロック境界ログに依存するケースです。LiteX の個別ケースは対応表と下記の回帰で確認します。

## RV32 の権限切り替えと仮想メモリ操作

上流の `tests/unit-tests/riscv-mret.robot` の `Should Invalidate TLB After Mode Change` は、同じ ELF と Robot 定義相当のメモリ配置で検証しました。ELF は Bare モードで、M モードから呼べた関数へのフェッチが `mret` で U モードに降りた後は PMP により拒否されます。修正前後とも、ステップ 42 で PC が禁止関数 `0x10068` に達し、ステップ 43 で命令をリタイアせず期待するトラップ先 `0x10000` に遷移しました。

```sh
out=$(mktemp -d)
robot=../renode/tests/unit-tests/riscv-mret.robot
url=$(rg -o 'https://[^[:space:]]+' "$robot")
curl -fsSL "$url" -o "$out/mret.elf"
printf '%s  %s\n' "${url##*-}" "$out/mret.elf" | sha1sum --check
cat > "$out/mret.repl" <<'REPL'
cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32im_zicsr"
ram: Memory.MappedMemory @ sysbus 0x10000
    size: 0x8000
forbidden: Memory.MappedMemory @ sysbus 0x2000
    size: 0x2000
REPL
target/release/penv run --platform "$out/mret.repl" --max-steps 43 --stats "$out/mret.elf"
```

上流の `tests/unit-tests/riscv-priv.robot` から RV32 の `Should Trap On Sret When MSTATUS.TSR Is One RV32`、対応する TSR=0 ケース、`Should Trap On SFENCE.VMA In User Mode RV32` を `privilege_controls_gate_sret_and_address_translation_operations` に移植しました。同ファイルの SATP と `sfence.vma` に対する TVM の RV64 ケースは、同じ権限規則を RV32 命令へ適用しました。TSR=1 の S モードで `sret` は違法命令になります。TVM=1 の S モードでは `satp` の読み書きと `sfence.vma` が違法命令になります。テストはトラップ時の `mcause`、`mepc`、`mtval` も確認します。

```sh
cargo test --locked
cargo build --locked --release
clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
  -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld \
  scripts/firmware/rv32-priv.S -o "$out/priv.elf"
target/release/penv run --platform ../renode/platforms/cpus/riscv_virt.repl \
  --max-steps 1000 "$out/priv.elf"
bash scripts/validate-riscv-upstream.sh target/release/penv ../renode "$out/litex"
```

TSR の検証 ELF は変更前の `66bb839` で失敗を UART に出力し、修正後は `rv32-priv: SRET with TSR trapped at the expected PC` を出力しました。両 ELF の指定ステップ到達時の終了コードは 2 です。Sv32 の既存ページテーブル再走査テストと LiteX Linux の回帰も確認しました。残課題は上流の RV32 権限スイート全体への拡張です。

### Sv32 有効時の TVM を実 ELF で検証

`scripts/firmware/rv32-tvm.S` は、上流の TVM=0/1 の 6 ケースを一つの RV32 ELF にまとめたものです。M モードで TVM を立て、VA `0x40000000` を PA `0x80000000` にマップしてから `mret` で S モードに入ります。TVM=1 の S モードでは `satp` の読み書きと `sfence.vma` の各命令で、違法命令の `mcause`、仮想 `mepc`、命令ビットの `mtval`、トラップ元の MPP を照合します。拒否された読み書きが宛先レジスターや `satp` を変えないことも確認します。M モードで TVM を解除し、再度 S モードに入ると、同じ命令列が完走して `satp` の新しい ASID を読み戻せます。TVM=1 の M モードでも `satp` アクセスと `sfence.vma` が通ることを検査します。

```sh
cargo test --locked
cargo build --locked --release
out=$(mktemp -d)
clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
  -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld \
  scripts/firmware/rv32-tvm.S -o "$out/tvm.elf"
status=0
target/release/penv run --platform ../renode/platforms/cpus/riscv_virt.repl \
  --max-steps 10000 "$out/tvm.elf" > "$out/uart.log" || status=$?
test "$status" -eq 2 &&
diff -u <(printf '%s\n' \
  'rv32-tvm: Sv32 supervisor SATP read/write and SFENCE.VMA gated; MRET recovery OK') "$out/uart.log"
```

TVM 実装前の `66bb839` は失敗マーカーを出し、`eced642` は成功マーカーを出しました。互換性の追加修正は不要でした。既存の TSR と virt のタイマー・UART 割り込みの ELF も再確認しました。上流 Robot スイートの直接実行は残課題です。ページテーブル更新後の TLB 無効化は次の ELF で検証します。

### Sv32 ページテーブル更新後の TLB 無効化

上流の `riscv-priv.robot` は `SFENCE.VMA` の実行可否を検査します。RV32 ELF ではこの権限ケースを拡張し、既存 Rust テスト `sfence_vma_rewalks_cached_mapping_after_pte_change` の再走査をゲスト命令だけで再現しました。S モードで同じ仮想ページのロードと命令フェッチを行い、両方で旧ページの値 `17` を確認します。次にゲストの `sw` でリーフ PTE を差し替え、`sfence.vma x0, x0` 後にロードと命令フェッチの両方が新ページの値 `29` を返すことを検査します。

```sh
cargo test --locked
cargo build --locked --release
out=$(mktemp -d)
sed 's/sfence.vma x0, x0.*/nop/' scripts/firmware/rv32-tlb.S > "$out/no-fence.S"
for variant in fence no-fence; do
  source=scripts/firmware/rv32-tlb.S
  if [[ $variant == no-fence ]]; then source=$out/no-fence.S; fi
  clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
    -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld "$source" -o "$out/$variant.elf"
  status=0
  target/release/penv run --platform ../renode/platforms/cpus/riscv_virt.repl \
    --max-steps 10000 "$out/$variant.elf" > "$out/$variant.uart" || status=$?
  test "$status" -eq 2 || exit 1
done
diff -u <(printf '%s\n' 'rv32-tlb: SFENCE.VMA refreshed Sv32 data and instruction translations') "$out/fence.uart"
diff -u <(printf '%s\n' 'rv32-tlb: stale translation or unexpected trap') "$out/no-fence.uart"
```

現行実装は両検査に合格し、追加の互換性修正は不要でした。fence を省いた対照 ELF は現行のキャッシュで古い変換を観測しました。fence を省いたときに古い変換が残るかは実装に依存します。既存の TVM ELF と上流の Bare/PMP ELF も再確認しました。アドレス・ASID を指定する fence の細粒度な無効化、複数ハート間の TLB シュートダウン、Robot スイートの直接実行は残課題です。

### RV32 カウンタと LiteX Tock の再現

```sh
cargo test --locked
cargo build --locked --release
out=$(mktemp -d)
clang --target=riscv32-unknown-elf -march=rv32imac -mabi=ilp32 \
  -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld \
  scripts/firmware/rv32-counters.S -o "$out/counters.elf"
target/release/penv run --platform ../renode/platforms/cpus/riscv_virt.repl \
  --max-steps 2000 "$out/counters.elf"
bash scripts/validate-riscv-upstream.sh target/release/penv ../renode "$out/litex"
```

RV32 ELF は 6 ケース通過と U モード権限検査通過の 2 行を出力し、待機ループのステップ上限で終了コード 2 になります。権限検査は M→S→U と遷移し、M モード CSR へのアクセスと無効化された `instret` 読み出しで違法命令トラップを確認します。その後、M モードで `mcounteren.IR` を有効化して U モードの `instret` 読み出しが通ることを確認します。Tock の検証は最初の `D1` を待つ上流と同様に、先行する `D2` を許容します。対象のケースはいずれも既存実装で通過しました。

### LiteX の SD rootfs と FAT16 の再現

`scripts/validate-riscv-upstream.sh` は上流 `litex_vexriscv_sdcard.resc` を使い、ext4 の root mount、`/sbin/init`、ログイン後の `linuxrc -> bin/busybox` を検査します。ここで使う上流デモの報告容量は 512 MiB です。FAT16 には `scripts/fixtures/litex-sd-fat.resc` を使い、上流 Robot と同じ 64 MiB の構成で `readme.txt` を読み、`readme2.txt` を書き込んで同期・再マウント後に読み戻します。書き込み先は実行中のオーバーレイであり、終了後にも元のカードイメージの SHA-1 が一致することを確認します。

### LiteX Linux 入出力と Nexys4DDR SPI SD の再現

```sh
cargo test --locked
cargo build --locked --release
bash scripts/validate-riscv-upstream.sh target/release/penv ../renode /tmp/penv-upstream
```

旧 LiteX Linux の起動は、機械モード SBI の `csrs sip, STIP` が無視され、スーパーバイザーへのタイマー通知が届かないため I²C 初期化後に停止していました。VexRiscv の内蔵割り込みコントローラーを使う M モードに限って、この SBI の `sip.STIP` セットとクリアを反映します。標準 RV32 と S モードの書き込み制限は Rust テストで確認しました。実ファームウェアはログイン、SPI ループバック、GPIO の消灯・点灯・消灯とボタンの `0 → 1 → 0` を通過しました。`--litex-led-assert` は REPL で配線された GPIO の LED も検証できます。`--litex-temperature-event STEP:NAME:CELSIUS` で `i2c.si7021` を 36 °C に変え、Linux が読む値が `4368 → 7724` になることを確認しました。

Nexys4DDR は変更前に `unknown LiteX SD controller "spisdcard"` で停止しました。LiteX SPI へのカード接続、初期化、単一ブロック読み出しを実装し、実 BIOS が `Q` で起動を中断した後、`spisdcardboot` で FAT16 の `README  .TXT` を列挙することを確認しました。SPI SD の書き込み、マルチブロック転送、入力 CRC 検査は残課題です。TFTP の再現では、ダウンロードした 218,608 バイトの MicroPython が UART からの式に `5` と応答します。

今回の優先範囲は、RV32 のカウンタと上記の LiteX 機能ケースです。全互換ケースの通過判定は保留です。RV32 アーキテクチャスイートには `riscv-arch-tests.robot` が指定するリビジョンのコンパイル済み ELF 一式（`RISCV_ARCH_TESTS_PATH`）が必要です。この環境では既定の `/riscv-arch-test/work` がありません。Robot の直接実行に必要な Renode Infrastructure サブモジュールは参照ツリーで未展開です。Verilated CFU は別途ネイティブモデルと実行アダプターが必要です。Microwatt は RV32 の対象外です。

### 公開 RV32 アーキテクチャ ELF の再現

[RISC-V Sail テストの 2026-08-20 リリース](https://github.com/riscv-software-src/sail-riscv-tests/releases/tag/2026-08-20) の prebuilt ELF を使います。実行用ツールは Python 3、`llvm-nm`、penv です。実行スクリプトは ELF の `tohost` シンボルを解決し、各 ELF の診断と結果を `results.jsonl` と個別ログに保存します。テスト終了を報告する書き込みが必要で、通常終了やステップ上限だけでは合格にしません。`--tohost` は奇数値の下位ワード書き込みで完了します。`--tohost-act` は上位ワードの書き込みを待ち、文字出力パケットと終了パケットを区別します。

```sh
cargo test --locked
cargo build --locked --release
out=$(mktemp -d)
release=https://github.com/riscv-software-src/sail-riscv-tests/releases/download/2026-08-20
curl -fsSL "$release/riscv-tests.tar.gz" -o "$out/riscv-tests.tar.gz"
printf '%s  %s\n' eb24265e83ca0b68a24a513dff7e4535975e724fe07d71f3cf75fef87fd75bec \
  "$out/riscv-tests.tar.gz" | sha256sum --check
mkdir "$out/isa"
tar -xzf "$out/riscv-tests.tar.gz" -C "$out/isa" --wildcards './rv32u[imac]-[pv]-*'
virt=../renode/platforms/cpus/riscv_virt.repl
sed '/cpuType:/a\    allowUnalignedAccesses: true' "$virt" > "$out/virt-unaligned.repl"
aligned=()
for elf in "$out"/isa/rv32u[imac]-[pv]-*; do
  [[ $elf == *-ma_data ]] || aligned+=("$elf")
done
runner=scripts/validate-rv32-architecture.py
python3 "$runner" --penv target/release/penv --platform "$virt" \
  --artifacts "$out/aligned" "${aligned[@]}"
python3 "$runner" --penv target/release/penv --platform "$out/virt-unaligned.repl" \
  --artifacts "$out/unaligned" "$out"/isa/rv32ui-[pv]-ma_data
```

物理モード `-p-` と仮想モード `-v-` を合わせて 122 件が通過しました。非整列の 2 件は設定を分けて実行しました。元の virt REPL では両 ELF がゲスト側の失敗コードを返し、非整列アクセスを有効にすると通過します。同じアーカイブから特権テストも実行できます。

```sh
mkdir "$out/priv"
tar -xzf "$out/riscv-tests.tar.gz" -C "$out/priv" --wildcards './rv32mi-*' './rv32si-*'
python3 "$runner" --penv target/release/penv --platform "$virt" \
  --artifacts "$out/priv-results" "$out"/priv/*
```

特権テストは 22 件すべて通過しました。`rv32mi-p-breakpoint` は変更前に `tohost` コード 2 で失敗しました。標準 RV32 に独立した 2 スロットのアドレス一致トリガーを実装した結果、命令実行・ロード・ストアのブレークポイントを検査する同じ ELF が `tohost` コード 0 を返します。

同リリースの `riscv-arch-tests.tar.gz`（SHA-256 `c07ba4f475e96395d77287e1fd471a4a43af47d09f121393f61812158462b3c1`）から `sail-rv32-max/elfs/rv32i/{I,M}/*.elf` を取り出し、同じ runner に `--act` を加えて実行しました。47 件すべてが初期化時の `fmv.w.x`（`mtval=0xf0008053`）で違法命令になり、ACT が失敗コード 1 を返しました。この配布物は Sail の最大構成で、Renode が指定する `spike-rv32-max` のビルドとは別です。penv の ISA 設定に合う ACT ELF を作るには、上流指定のソースと参照モデル・ツールチェーンが必要です。この環境では Sail と RISC-V GNU クロスコンパイラが未導入です。

Renode の例外ケースは Robot 内の URL から `riscv32--ebreak_custom_test.elf` を取得し、SHA-1 `4db0870a69de9bba7ccda18908832c5b72cff35e` を照合して実行しました。REPL は `CPU.VexRiscv` の `rv32im_zicsr`、RAM `0x80000000`（`0x1000` バイト）、LiteX UART `0x40008000`（`txFifoCapacity: 0`）を指定します。上流と同様に RESC の `sysbus LoadELF` からロードすることで ELF の浮動小数点 ABI フラグを受け入れます。例外ケースと LiteX の直接起動 MicroPython は `scripts/validate-riscv-upstream.sh` で再現できます。MicroPython は上流 RESC を実行し、UART の `2 + 3` → `5` を照合します。

### RV32 優先ケースの追加

| 分野 | 出典と対応 |
| --- | --- |
| ISA | Renode の `unit-tests/riscv-zicond-extension.robot` の RV32 ゼロ化・通過・無効時の違法命令、および全ビットが 1 の値の保持を Rust テスト `zicond_rv32_matches_zero_nonzero_and_disabled_cases` に移植。公開済み Zicond ELF 4 件も実行 |
| 特権 | 公開 `rv32mi-*` / `rv32si-*` と、上流 `riscv-priv.robot`・`riscv-mret.robot` に対応する既存 ELF。`rv32mi-p-breakpoint` は実行・読み出し・書き込みのアドレス一致トリガーを検証 |
| Sv32 | 公開 `rv32si-p-dirty`、仮想モードの I/M/A/C ELF、既存の PTE 差し替え ELF。SUM と MXR を変えたときに、キャッシュ済み変換が許可を持ち越さないことを Rust テストで追加検証 |
| CLINT | Renode `unit-tests/timer-reconfiguration.robot` の比較値の上下ワード更新から派生し、32 ビット境界をまたぐ `mtimecmp` 比較を追加検証。10 MHz の実割り込みは virt ELF で再確認 |
| PLIC | 上流 `platforms/cpus/riscv_virt.repl` の 52 入力・M コンテキストに合わせ、ソース 52、しきい値と同値での通知抑止、claim/complete を Rust テストで検査。ソース 3 は virt の NS16550 実 ELF で再確認 |

Zicond は REPL の対応済み RV32 ISA 文字列に `_zicond` を付けると有効になります。変更前の `aaa317e` はこの CPU 型を拒否しました。変更後は `czero.eqz` と `czero.nez` の物理・仮想 ELF がすべて通過しました。上記の公開 ELF アーカイブと LLVM の `llvm-nm` を使うため、この 4 件の実行に Sail やクロスコンパイラーによる再生成は不要です。

```sh
cargo test --locked
cargo build --locked --release
mkdir "$out/zicond"
tar -xzf "$out/riscv-tests.tar.gz" -C "$out/zicond" --wildcards './rv32uzicond-*'
sed 's/"rv32imac"/"rv32imac_zicsr_zifencei_zicond"/' "$virt" > "$out/virt-zicond.repl"
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$out/virt-zicond.repl" --artifacts "$out/zicond-results" "$out"/zicond/*
```

公開 ELF の通過内訳は I/M/A/C 122 件、M/S 特権 22 件、Zicond 4 件です。virt の NS16550 と CLINT の実 ELF および LiteX の実ファームウェア回帰は、それぞれ上の再現手順を使います。残課題はデバッグモードへの遷移・連鎖・範囲一致のトリガー、RV32 の未対応 ISA 拡張、対応 ISA に絞った ACT ELF の生成、Robot の翻訳ブロックログやネイティブ周辺機器を必要とするケースです。たとえば上流の Verilated UARTLite や Andes 固有 PLIC は、ここで検証した標準 virt の PLIC と構成が異なります。

### RV32 virt と LiteX の受け入れ検証

`close-rv32-isa` の優先ケースと `close-litex` の対象ケースを一つのコマンドで確認します。

```sh
bash scripts/validate-riscv-acceptance.sh ../renode "$(mktemp -d)"
```

第 3 引数には上記リリースの保存済みアーカイブを渡せます。依存は Rust、GNU tar、curl、Python 3、Clang、LLD、`llvm-nm`、ripgrep と公開イメージへのアクセスです。`verification.log` に両リビジョン、実行バイナリの SHA-256、結果を保存します。Cargo のログ、148 件の ELF の結果と個別ログ、実ファームウェアの UART・PCM・画像比較の証拠は同じディレクトリに置かれます。

| 受け入れ項目 | 判定 |
| --- | --- |
| ISA・特権・Sv32 | 公開 ELF 148 件の `tohost=0`、TSR・TVM・PTE 差し替え・カウンタの実 ELF の UART 一致、上流 Bare/PMP MRET ELF の最終 PC |
| CLINT・PLIC・UART | 上流 virt REPL で 10 MHz タイマー割り込みと PLIC 経由の NS16550 受信割り込み、Rust の比較値境界・claim/complete |
| LiteX Linux・Minerva・Arty | 単一ハートと SMP のログイン、Minerva UART ブート、Arty LED と実行中の PHY 設定を公開イメージで確認 |
| LiteX I²S・Nexys・SD | Zephyr の PCM バイト一致、Nexys Video の上流 PNG 一致・ROI 一致と不一致、Nexys4DDR SPI SD の FAT16 列挙 |
| 追加の実ファームウェア回帰 | Murax、Ibex、Zephyr シェル、Tock、Linux GPIO/SPI/I²C、SD root/FAT16、TFTP、MicroPython、RV32 ECALL/EBREAK |

上流 Robot 一式の実行、Renode 指定の ACT ビルド、サポート外 ISA 拡張とネイティブモデルは引き続き残課題です。

## Renode / penv 共通計測ハーネス

`python3 scripts/benchmark-riscv.py manifest.json 新規出力ディレクトリ` は Linux の GNU `/usr/bin/time` を使います。現在の動作確認は `python3 scripts/test-benchmark-riscv.py` のダミープロセスのみです。実機比較は上流互換テスト全件通過後に行います。

```json
{
  "firmware": "assets/demo.elf",
  "input": "assets/uart.bin",
  "uart_marker": "DEMO COMPLETE",
  "timeout_seconds": 60,
  "repeats": 5,
  "engines": {
    "renode": {"argv": ["./run-renode", "{firmware}", "{input}"]},
    "penv": {"argv": ["./run-penv", "{firmware}", "{input}"], "success_exit_code": 2}
  }
}
```

パスと各エンジンの任意の `cwd` はマニフェストの場所が基準です。両コマンドに同じ絶対パスを渡してファイルの SHA-256 を記録します。コマンドは UART を標準出力に書き、ワークロード終了後に指定の終了コードで終了するよう構成します。マーカーは UTF-8 のバイト列として比較し、経過時間はプロセスの起動から終了までです。試行は直列で、各回のエンジン実行順を交互に入れ替えます。

`trials.jsonl` に各試行の成否、ログの場所、取得できた経過秒と最大 RSS（KiB）を記録します。タイムアウト時はプロセスグループを停止します。`summary.json` の最小値・中央値・最大値には、指定終了コードで終了し、マーカーと測定値を取得できた試行だけが入ります。各エンジンの有効試行が 2 件未満ならハーネスは終了コード 1 を返します。

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

最初の 6 件は `tests/platforms/STM32F072b.robot`、最後の 1 件は `tests/unit-tests/hex.robot` です。固定ツリーの `scripts/` に STM32F072 専用の RESC はなく、Robot の起動・操作を `tests/fixtures/stm32f072-*.resc` に転記しました。全ケースで上流の変更していないボード REPL と公開イメージを使用します。

| 上流ケース | 移植状況 | 観測条件 |
| --- | --- | --- |
| `Should Run Zephyr Hello Sample` | 今回追加。 `upstream_stm32f072_hello_world_boots_elf_and_hex` の ELF 実行 | `Hello World! stm32f072b_disco` の完了行。 |
| `Should Run Zephyr Console Echo Sample` | 既存。 `upstream_stm32f072_console_echo_accepts_resc_uart_input` と `upstream_stm32f072_console_echo_replies_after_uart_prompt` | 入力案内を受けて送信し、次に `Echo working?` を受信。 |
| `Should Run Zephyr Blinky Sample` | 移植済み。 `upstream_stm32f072_blinky_alternates_led_states` | LED の点灯・消灯を 1 秒ずつのタイムアウトで 3 往復確認します。同じ ELF で SysTick のクロック停止・再開も別途検査します。 |
| `Should Run Zephyr Button Sample` | 今回追加。 `upstream_stm32f072_button_updates_led_and_reports_press` | 3 回の押下・解放、数値の押下時刻を含む UART 行、LED の遷移列。期待状態に達してからの 1 秒間を各操作で検査し、途中で LED が変化すれば失敗します。 |
| `Should Read ADC` | 既存。 `upstream_stm32f072_adc_waits_for_uart_between_voltage_changes` | 起動行、0.6 V で `745`、1.2 V で `1489`。 |
| `Should Run stm32f0-crc-test Application` | 今回追加。 `upstream_stm32f072_crc_firmware_passes_840_cases` | `test result: ok. 840 passed; 0 failed` の完了行。 |
| `Should Load HEX` | 今回追加。 `upstream_stm32f072_hello_world_boots_elf_and_hex` の HEX 実行 | `LoadHEX`、明示的な `VectorTableOffset`、同じ Hello World 完了行。 |

対応する RESC は順に `stm32f072-hello-elf.resc`、`stm32f072-echo.resc`、`stm32f072-blinky.resc`、`stm32f072-button.resc`、`stm32f072-adc.resc`、`stm32f072-crc.resc`、`stm32f072-hello-hex.resc` です。追加の公開 ELF 回帰では、Echo の `stm32f072-prompt.resc` と Blinky の `stm32f072-blinky-clock.resc` を使います。

`tests/unit-tests/per-core-registration.robot` の `Should Load Hex To Core Specific Memory` と `Should Not Load Hex To Invalid Core Specific Memory` は同じ HEX を参照します。CPU ごとのメモリ登録とロード先指定が必要なため、上表の単一 CPU の HEX 回帰とは別の残課題です。

Blinky と Button の RESC が使う `waitForLed sysbus.gpioPortC.led true "1s" ["1s"]` は、タイムアウト内に指定状態を待ち、末尾の期間を指定した場合はその状態を保持することを検査します。LED 名と真偽値は各操作で指定し、経過したステップはスクリプト全体の上限に含まれます。対象は GPIO に配線した STM32 ボード LED です。Robot のテスター ID、デューティー比計測、シフトレジスター接続 LED の待機は残課題です。[Renode LEDTester](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Main/Testing/LEDTester.cs)

#### Cortex-M と STM32 周辺機器

| 分野 | 上流のケース | penv の対応と検証範囲 |
| --- | --- | --- |
| Thumb-2 | `tests/unit-tests/tlib/arm/thumb2-data-processing-instr.robot` の `Should Run All Instructions` | `renode_thumb2_fixture_branches_and_reads_apsr` に ELF の `BL`（`0x1002`、`0x1006`）と `MRS APSR`（`0x1a1e`）の命令語を移植しました。分岐先と LR を検査し、ORRS ケースが期待する N/Z/C を事前設定して APSR の読出しを検査します。 |
| Thumb 状態 | `tests/unit-tests/arm-thumb.robot` の `Should Expose Thumb State In CPSR`、`tests/unit-tests/arm-thumb2-srs-rfe.robot` の SRS/RFE | Cortex-A の ARM/Thumb 切り替え、CPSR、SRS/RFE は対象外です。既存の `runs_cortex_m0_from_vector_table_and_reports_invalid_thumb_state` はリセットベクターの Thumb ビットを検査します。 |
| 例外復帰 | `tests/unit-tests/arm-cortex-m-exception-return.robot` の `Should Return From Exception When Using Allowed Instruction` | 非 TrustZone の `BX LR` を `renode_exception_bx_lr_restores_interrupted_thread` に移植しました。上流の NMI を Cortex-M0 の外部 IRQ に置き換え、ハンドラーへの遷移とスレッドの PC・SP の復帰を検査します。TrustZone、MPU、HardFault のケースは見送りました。 |
| 例外とイベント | `tests/unit-tests/arm-cortex-m-interrupts.robot` の `Should Wake Up From WFE When SEVONPEND Is Set` | 移植済み。 `renode_wfe_wakes_on_new_pending_irq_only_with_sevonpend` は上流と同じ IRQ 5、書込みアドレス、データを使い、SEVONPEND の有無を比較します。`renode_resc_nvic_gpio_wakes_wfe_with_sevonpend` は RESC の `OnGPIO` で IRQ 5 を操作し、RAM への書き込みの代わりに UART で復帰を観測します。 |
| SysTick | 同ファイルの `Should Count Down SysTick With Reload Set Before Reset` と `Should Count Down SysTick With Reload Set After Reset` | 今回追加。 `renode_systick_counts_and_reloads_for_both_cvr_write_orders` で CVR/RVR の両順序、カウントダウン、COUNTFLAG の読出しによる解除、再ロードを検査します。100 Hz、リロード値 10、上流と同じ 0.09 秒・0.01 秒・0.01 秒の区間で CVR が 10 → 1 → 10 → 9 になることを検査します。 |
| NVIC のリセット | 同ファイルの `NVIC Base Priority Mask Setting Should Not Survive Reset` | 対象外。BASEPRI は現行の Cortex-M0/M0+ の範囲外です。 |
| Fault、クロック信号 | `tests/unit-tests/arm-cortex-m-bus-fault.robot` と `tests/unit-tests/arm-cortex-m-signals.robot` | SysTick のクロックゲートとそのリセット時の保持を移植しました。CPU/DWT の外部クロックゲートと Fault の例外配送・ロックアップは未移植です。既存の CLI テストは無効なリセットベクターをエラーとして検査します。 |
| UART 受信 | `tests/peripherals/STM32_UART.robot` の `Should Drop Characters When UART Or Receiver Disabled` | `renode_stm32_usart_drops_input_if_either_enable_is_clear` は F072 のレジスター配置で、上流の `0x12` と `0x34` が受信器または USART の無効中に破棄され、再有効化しても現れないことを検査します。フレーム時間、パリティ、アイドル検出の上流ケースは対象外です。 |
| UART 端末 | `tests/platforms/STM32F072b.robot` の Console Echo と ADC、`tests/unit-tests/uart-tester.robot` の端末選択 | 既存の公開 ELF 回帰は Echo、プロンプト待機、ADC をカバーします。`uart_watch_ignores_other_ports_without_filtering_stdout` は端末選択に対応する USART 分離を検査します。Robot のテスター ID とバイナリ・正規表現モードは未移植です。 |
| STM32 フラッシュ | `tests/unit-tests/stm32f4-erase.robot` の `Should Ignore Erase When MER And SER Are Reset`、`Should Perform Mass Erase`、`Should Perform Sector Erase`。隣接ページの観測条件は `tests/platforms/STM32L072.robot` の `Should Handle Flash Operations` | `renode_stm32_flash_erase_modes_preserve_the_expected_ranges` は F072 の 128 KiB バンク、2 KiB ページ、消去値 `0xff` を使用します。消去モードなし、指定ページ全体と隣接ページ、バンク全消去を検査します。F4/L0/H7 固有のコントローラーとフラッシュ用 ELF は対象外です。既存の `stm32f072_flash_preserves_programmed_data_on_reload_and_erases_a_page` は F072 のプログラムと再ロードを検査します。 |
| ADC | `tests/platforms/STM32F072b.robot` の `Should Read ADC` | 公開 ELF の UART 回帰に加えて、`renode_stm32f072_adc_sample_values_drive_eoc_irq` で上流 REPL と同じ 3.3 V 基準とチャネル 1 を使い、`745`、`1489`、EOC 割り込み要求、DR 読み出し後の解除を検査します。 |
| GPIO と EXTI | `tests/platforms/nucleo_wba52cg.robot` の `Should have Working EXTI`、F072 の `Should Run Zephyr Button Sample` | `renode_button_press_and_release_latches_exti_between_steps` に移植。 `sysbus.gpioPortA.button PressAndRelease` が CPU ステップを挟まずに押下・解放しても、EXTI が設定済みのエッジを保留し、NVIC に配送します。公開 F072 Button ELF の押下・解放と LED 保持も引き続き検査します。 |
| TIM2 更新 | `tests/platforms/STM32F4_Discovery.robot` の `Should Fire Update Event When Counting Up` と `Should Fire Update Event When Counting Down` | `renode_tim2_update_irq_repeats_in_both_directions` に更新条件を移植。 縮尺したプリスケーラーと ARR を使い、両方向で 3 周期の UIF、IRQ、クリアと再発行を検査します。 |
| GPIO の PWM | `tests/platforms/STM32L072.robot` の `PWM Should Support GPIO Output` | `renode_stm32_pwm_duty_reaches_gpio_for_all_four_upstream_pulses` に CH3 の周期 256 とパルス幅 5、85、127、250 を移植。 F072 の TIM2 と GPIO の接続からハイ期間を数え、4 条件すべてで上流の許容差 10 ポイントを満たします。L072 専用 RESC・PWMTester・シェルコマンドは残件です。 |
| PWM 入力キャプチャ | `tests/platforms/STM32F4_Discovery.robot` の `PWM Capture` と `PWM Capture Four Channel Capture Support`、`tests/platforms/nucleo_wba52cg.robot` の `Timer Should Support Input Capture` | `renode_stm32_pwm_input_capture_routes_period_and_pulse_to_gpio` は、GPIO から CH1/2 と CH3/4 の各キャプチャペアに 1 kHz・33% の信号を入力し、周期とパルス幅を検査します。元のボードでの配線追加、WBA 固有の 2/3 kHz 設定と UART 報告、Cortex-M4/M33 ELF の起動は残件です。 |

STM32 フラッシュの消去操作は Rust のバス回帰で検証しています。

固定上流の `Button.PressAndRelease` は連続する `Press` と `Release` です。GPIO のエッジは EXTI で即時に保留し、次の実行ステップで配送します。[固定上流の Button 実装](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Miscellaneous/Button.cs)

タイマー回帰の残件は、STM32F4 の直交エンコーダー、FAULTMASK を使う割り込み、STM32L072 の低消費電力タイマーと RTC の専用サンプルです。F4 の公開 TIM2 アップカウント ELF を固定上流の `platforms/cpus/stm32f4.repl` で試すと、10 行目の `MTD.STM32F4_FlashController` が未対応で起動前に止まります。上の移植は実行可能な観測条件を F072 向けに抽出したものです。固定ツリーの `tests/unit-tests/timer-reconfiguration.robot` は RISC-V CLINT と変換ブロックのケースなので、この ARM 回帰一覧の対象外です。

#### NVIC・SysTick・例外の追加移植

同じ固定リビジョンで、前回の `BX LR`、`SEVONPEND`、SysTick の CVR/RVR 書き込み順の回帰に加え、次の観測条件を移植しました。

| 上流のケース（`tests/unit-tests/` 内） | 追加した Rust テストと差分 |
| --- | --- |
| `arm-cortex-m-exception-return.robot` の `Should Not Return From Exception When In Thread Mode` と `Should Not Return From Exception When Not Using Allowed Instruction` | `renode_thread_bx_and_handler_blx_do_not_unstack` は、スレッドの `BX` とハンドラーの `BLX LR` が Cortex-M0 の `0xfffffff9` を分岐先として扱い、例外フレームを解除しないことを検査します。TrustZone の `BLXNS` は対象外です。 |
| 同ファイルの `Should Not Return From Exception When Loading PC From Vector Table` | `renode_exception_vector_load_does_not_unstack_an_exc_return_value` は、上流の MPU/HardFault の準備を外部 IRQ に置き換えます。ベクター内の `0xfffffff9` が PC にロードされ、IRQ がアクティブでスタックが保持されることを検査します。 |
| `profiler-trace.robot` の `Should Disable Profiler After Reaching Maximum Nested Context Count` | `renode_profiler_fixture_pendsv_dispatches_from_icsr` は、先頭の PendSV を抜き出し、上流と同じ ICSR とベクター・ハンドラーのアドレスで、要求の設定・取り消し・受理・復帰を検査します。MPU の MemManageFault とプロファイラーの判定は対象外です。 |
| `cpu-pause-halt.robot` の `CPU Should Halt and Reset on Cortex-M` と `CPU Should Halt Pause and Reset on Cortex-M` | `renode_cortex_m_reset_observation_via_sysresetreq` は、CPUWAIT やモニターの停止操作を WFI と AIRCR に置き換えます。VECTKEY の検証、リセットベクターへの復帰、NVIC と SysTick レジスターの初期化、RAM の保持を検査します。 |
| `arm-cortex-m-signals.robot` の `Should Stop NVIC SysTick and DWT CycleCounter When Not Clocked` | `renode_systick_counter_stops_and_resumes_via_enable` は、SysTick の観測条件を CSR.ENABLE で検査します。カウンターが停止中に変化せず、再開時に残っていたティック位相から進むことを確認します。外部クロックゲートと DWT は対象外です。 |

#### 対象ケースの最終突合

STM32F072B Discovery の直接対象は上表の 6 件と HEX 起動 1 件です。Cortex-M の表で未移植だった観測条件を再確認し、現行の単一 Cortex-M0/M0+ で実行できるものを追加しました。

| 上流ケース | 通過した観測条件と残件 |
| --- | --- |
| `arm-cortex-m-signals.robot` の `Should Stop NVIC SysTick and DWT CycleCounter When Not Clocked` | SysTick 部分を移植。 `renode_systick_external_clock_gate_survives_reset` は CSR.ENABLE を保った状態で外部クロックを停止します。 `upstream_stm32f072_blinky_pauses_while_systick_is_not_clocked` は公開 ELF で点灯後にクロックを止めて 1 秒間点灯を保持し、再開後に消灯することを確認します。DWT のサイクルカウンターと独立制御は残件です。 |
| 同ファイルの `Should Persist Clocked State For Cortex-M Complex On Reset` | SysTick 部分を移植。 同じ Rust テストで、リセット後の CVR が `0x00ffffff`、ゲートが設定されたままで、ゲストが再有効化しても凍結し、クロック再開で所定の周波数で進むことを確認します。CPU と DWT のクロック、および CPUWAIT のケースは残件です。 |
| `arm-cortex-m-exception-return.robot` の `Should Not Return From Insecure Mode When Not Using TrustZone` | 移植済み。 `renode_nonsecure_return_token_branches_without_trustzone` は上流と同じ `0xfeffffff` と `BX r0` を使い、PC が `0xfefffffe` になり SP と例外状態が維持されることを確認します。TrustZone を有効にする `BXNS`、`BLXNS`、FNC_RETURN は残件です。 |

RESC の `sysbus.nvic Clocked False` と `sysbus.nvic Clocked True` は、REPL 名で選択した NVIC の SysTick クロックを停止・再開します。再現は `cargo test renode_systick_external_clock_gate` と `cargo test renode_nonsecure_return_token`、および次節の公開イメージ回帰を使います。

残件の実装依存は、`arm-cortex-m-interrupts.robot` の BASEPRI、`it-status.robot` の IT 状態を含む例外フレーム、`arm-cortex-m-bus-fault.robot` の Fault 配送とロックアップ、`per-core-registration.robot` の CPU 別メモリ登録です。これらのケースの成功は、上表の既存機能の回帰結果には含めていません。

SysTick の 100 Hz ケースでは、固定した上流の NVIC と同じくカウントがゼロに達したティックで再ロードします。CVR への書き込みは直ちに RVR の値を読み込み、COUNTFLAG をクリアします。RVR がゼロの場合は満了後に停止します。`cargo test renode_systick` で両方の書き込み順と、ゼロリロード・CVR 書き込み時に割り込みが発生しない条件を確認できます。[固定上流の NVIC 実装](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Cores/Arm-M/NVIC.cs)

RESC の `sysbus.nvic OnGPIO 5 true` と `sysbus.nvic OnGPIO 5 false` は、REPL で宣言された NVIC の外部 IRQ（0〜31）を操作します。`true` の直後に `false` を実行しても、CPU が次のステップに進む前に要求が保留されます。`cargo test renode_resc_nvic_gpio` で WFE の復帰、信号が非アクティブな場合のタイムアウト、REPL に存在しない名前のエラーを再現できます。[固定上流の NVIC.OnGPIO](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Cores/Arm-M/NVIC.cs#L157)

通常の `cargo test` は 76 件成功しました。追加の GPIO・タイマー・ADC の移植は `cargo test renode_`、LED の失敗経路は `cargo test led_wait_times_out` で確認できます。

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

公開イメージによる回帰は 8 テストで、Hello World の ELF/HEX の 2 形式、Console Echo の 2 経路、ADC、Blinky の通常動作とクロック停止・再開、Button、CRC の計 9 シナリオを確認しました。

#### STM32F072 の受入証跡

受入対象は、STM32F072B Discovery の Robot 6 ケースと HEX 起動 1 ケース、および同じ公開イメージを使うプロンプト・クロック停止の追加回帰です。上の対応表にある他 SoC、CPU 別メモリ登録、Cortex-M の未対応機能は残課題です。

```sh
evidence="$(mktemp -d)/arm-acceptance"
scripts/verify-stm32f072.sh ../renode "$evidence"
```

スクリプトは、クリーンな追跡済みファイルと固定 Renode リビジョンを確認し、上流 Robot と RESC の参照 URL が一致することを検証します。配布名に含まれるサイズと SHA-1 を照合してから `cargo test --locked` と公開イメージ回帰を実行し、期待する 8 テストがすべて合格したことを名前で照合します。証跡には、両リビジョン、ファームウェア SHA-256、`cargo-test.log`、`stm32f072.log`、合格テスト一覧が含まれます。この環境で `cc` がなければ、`CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc` を指定できます。

F4 の TIM2 ELF が起動前に止まる箇所は以下で再現できます。F072 のタイマー観測条件は `cargo test renode_tim2` と `cargo test renode_stm32_pwm` で実行します。

```sh
cargo build --release
f4_url=$(sed -n 's/^.*sysbus LoadELF @\(https:.*stm32f4disco-timer-upcount.*\)$/\1/p' \
  "$renode_root/tests/platforms/STM32F4_Discovery.robot")
curl -fL "$f4_url" -o "$assets/f4-tim2-up.elf"
target/release/penv run --platform "$renode_root/platforms/cpus/stm32f4.repl" \
  --platform-root "$renode_root" --max-steps 1000000 "$assets/f4-tim2-up.elf"
```

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
