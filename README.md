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

機種固有スイートと共有 Robot/NUnit の棚卸し、および固定 ACT/ISA の実行は [RV32 上流互換検証](docs/riscv-full-upstream.md) に記録します。

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

`WriteLine` は REPL 名で STM32 USART を選択し、Renode の既定と同じ CR を付けて受信 FIFO に入力します。この回帰では `waitForUart` の対象を `sysbus.usart1` に指定して入力案内を待ち、`sysbus.usart1 WriteLine "Echo working?"` で CR を送ります。その後、同じ USART の `waitForUartPrompt` で返答を待ちます。両方の待機に上流と同じ `timeout=8s` を指定します。CLI で入力ステップを指定せずにシナリオを再現できます。

```sh
assets=$(mktemp -d)
echo_url=$(sed -n 's/^sysbus LoadELF @//p' tests/fixtures/stm32f072-echo.resc)
curl -fL "$echo_url" -o "$assets/${echo_url##*/}"
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS="$assets" \
  cargo test --release upstream_stm32f072_console_echo_accepts_resc_uart_input -- --ignored
```

公開 ELF で案内の次の行が `Echo working?` になり、終了コード 0 を返すことを確認しました。公開 ADC ELF の既存回帰と Hello World HEX の RESC 起動も再確認しました。通常の `cargo test` は 48 件成功しました。これには名前で選んだ USART だけに本文と CR が届くテストと、フラッシュのロック、半ワード書き込み、オプション再ロードでの保持、ページ消去の確認が含まれます。

`waitForUart sysbus.usart1 "期待文字列"` は、指定した USART が送信した LF または CRLF 終端の行だけを照合します。標準出力にはすべての UART 出力が流れます。USART を省略した従来の形式は、その標準出力を照合します。

`WriteLine "文字列"` の後ろに `CR`、`LF`、`CRLF`、`None` を指定できます。省略と `true` は CR、`false` は行終端なしです。Console Echo の公開 ELF で既定の CR と補助テストの明示的な CRLF を、ADC の公開 ELF で USART を指定した `Booting Zephyr` → `745` → `1489` の待機を確認しました。両 ELF を用意済みの場合は、上のテスト名を `upstream_stm32f072` にすると両方を実行できます。通常の `cargo test` は 50 件成功し、別ポートからの同一文字列と行終端引数の全指定を検査します。

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
python3 scripts/verify-rv64-virt.py "$work" --logs target/rv64-virt-verification
```

検証スクリプトは UART で公開デモの `root` / `starfive` を使って認証し、`id -u` と `uname -m` の出力を照合します。initramfs の起動スクリプトは `eth0` を探してタイムアウトします。この virt 構成の残課題は VirtIO、PCIe、RTC、SiFive Test のリセットと電源断、QEMU の `a2` を使う FW_DYNAMIC、複数ハートの Linux 検証です。PLIC の閾値と MMIO 範囲には既存の Renode 向けモデルを使用します。

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

全上流互換の作業状況は [RV64 上流ケース台帳](docs/rv64-full-upstream.md) に記録しています。下表はこれまでの優先ケースに対応します。

棚卸しの参照点は `../renode` の `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` です。上流の `platforms/cpus/riscv_virt.repl` は RV32 定義で、このリビジョンの Robot スイートには RV64 virt の Linux 起動ケースが見当たりません。RV64 virt の実ファームウェア検証は上記の OpenSBI FW_JUMP 手順で行い、RV64 CPU に共通する次のケースを Robot から Rust に移植しました。上流の `VirtioMMIOConsole.robot` は Cortex-A53/Zephyr を対象とします。[Renode riscv_virt.repl](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/riscv_virt.repl) [Renode VirtioMMIOConsole.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/peripherals/VirtioMMIOConsole.robot)

| 上流ケース | 移植先と確認内容 |
| --- | --- |
| `riscv-priv.robot` の `Should Increment MCYCLE RV64` / `Should Increment MINSTRET RV64` | `increments_mcycle` / `increments_minstret`。CSR 読み出し間の差分が 2 |
| 同 `Should Inhibit MCYCLE RV64` / `Should Inhibit MINSTRET RV64` | `inhibits_mcycle` / `inhibits_minstret`。抑止中の差分が 0 |
| 同 `Should Inhibit MCYCLE After Write Finishes RV64` / `Should Inhibit MINSTRET After Write Finishes RV64` | `inhibits_mcycle_after_write_finishes` / `inhibits_minstret_after_write_finishes`。抑止を書き替える命令を境界にした差分が 2 |
| 同 `Should Trap On SFENCE.VMA In User Mode RV64` | `traps_sfence_vma_in_user_mode`。例外の入口、原因 2、`mepc` |
| 同 `Should Fail On Counter Access` | `traps_on_counter_access_from_user_mode`。ユーザーモードからの `mcycle` と `instret` を拒否 |
| 同 `Should [Not] Trap On Sret When MSTATUS.TSR Is {One,Zero} RV64` | `sret_obeys_tsr_in_supervisor_mode`。例外と `sepc` への復帰 |
| 同 `Should [Not] Trap On {SATP Write,SATP Read,SFENCE.VMA} When TVM Is {One,Zero}` | `supervisor_satp_and_sfence_obey_tvm`。3 命令それぞれの許可と例外 |
| `riscv-amocas.robot` の `Amocas.{w,d,q} On RV64 Should Set Value At Single-Page Memory Location` | `amocas_{w,d,q}_changes_single_page_on_match`。旧値の返却、置換、アドレス・置換レジスター、書き込み幅 |
| 同 `Amocas.{w,d,q} On RV64 Shouldn't Set Value At Single-Page Memory Location` | `amocas_{w,d,q}_preserves_single_page_on_mismatch`。旧値の返却とメモリ保持 |
| 同 `Amocas.q On RV64 Should Handle Zero {Source,Destination} Register` / `Amocas.q On RV64 Using Odd Registers Should Throw Illegal Instruction` | `amocas_q_zero_*` / `amocas_q_traps_on_odd_register_pairs`。`x1` とメモリの保持、奇数レジスター対の例外 |

Rust 移植は `src/rv64/upstream_tests.rs` にあります。Monitor のレジスター設定と Step を Rust のレジスター初期化と実際の命令実行に置き換えました。`amocas.w` は旧値の下位 32 ビットに加えて、RV64 での符号拡張を検証します。対象はこの表の上流 25 ケースです。Renode の `allowUnalignedAccesses: true` を前提とする AMOCAS のページ境界またぎ、`Memory.ArrayMemory` を使う MMIO、他の AMO と汎用 ISA スイート（`riscv-isa-tests.robot` と `riscv-arch-tests.robot`）は残課題です。[Renode riscv-priv.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/riscv-priv.robot) [Renode riscv-amocas.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/riscv-amocas.robot)

| プラットフォームの検証ケース | CLI での対応 |
| --- | --- |
| StarLight `Should Boot U-Boot` | 両ゲストの UART3 で OpenSBI v0.9、StarFive、U-Boot 2021.01、`dwmac.10020000` を順に照合 |
| StarLight `Should Provide Two Linux Machines With Ethernet Connection` | U-Boot 確認後にローカルスイッチを起動し、両ゲストにログインして上流と同じ MAC・IPv4・MTU 440 を設定 |
| StarLight `Should Ping` | B から A に 56 バイトを 10 回送り、全応答と損失 0% を確認 |
| StarLight `Should Ping Large Payload` | B から A に 3200 バイトを 10 回送り、全応答と損失 0% を確認 |
| StarLight `Should Boot U-Boot With External PMP` / `Should Provide Two Linux Machines With Ethernet Connection With External PMP` / `Should Ping With External PMP` | 上流と同じ外部 PMP を両 U74 に接続。2 台の起動、ログインと 56 バイト ping の 10/10 応答を確認 |
| StarLight の標準 `reboot`（Penv 回帰） | GPIO 再起動ドライバーを有効にした Linux で `--recovery --linux-reboot`。ゲストの再起動、OpenSBI 再表示、UART 再認証、ローカル Ethernet 復帰を確認 |
| RV64 virt の Linux 起動（Penv 回帰） | `verify-rv64-virt.py`。OpenSBI v1.6、Linux 5.10.6 の `ttyS0`、UART の root 認証とコマンド出力を確認 |
| Fire `Should Boot Buildroot From SD Card` | `verify-fire.py` で通過。公開 HSS と SD を元の RESC で起動し、OpenSBI v1.2、U-Boot 2023.07.02、Linux 6.6.35、UART の root ログイン、`uname -m` と `uname -n` を確認。さらにゲストから SD の先頭セクターを読み、公開イメージとの SHA-256 一致を確認 |

StarLight の上流ケースは `verify-starlight.py --upstream-cases` で実行します。上流と同じ公開 RESC・ファームウェアを使い、ケース名ごとに結果を出力します。Penv のゲストプロセスは U-Boot 確認とスイッチ接続の間も動作を続けます。Linux の追加コマンドと `eth0` IRQ は、同じスクリプトで `--upstream-cases` を省いた診断モードで検証します。[Renode BeagleV-StarLight.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/BeagleV-StarLight.robot) [Renode BeagleV-Fire.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/BeagleV-Fire.robot)

`monitor-tests.robot` にも Fire の REPL を使うケースがあります。配列プロパティ設定、`Select` の整形、スカラー型エラーを検証する Monitor ケースなので、UART 起動や RV64 CPU の優先ケースとは別に保留しました。[Renode monitor-tests.robot](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/monitor-tests.robot)

以下では公開 StarLight 素材を `$assets`、再起動用 FIT のビルド先を `$out`、RV64 virt の起動素材を `$work`、公開 Fire 素材を `$fire_assets` に用意します。素材の取得と作成方法は各プラットフォームの節にあります。StarLight のスイッチはループバックだけを使います。

```sh
cargo test
cargo build --release
logs=target/close-rv64
python3 scripts/verify-starlight.py "$assets" --ports 30801 30802 \
  --switch-port 30800 --upstream-cases --logs "$logs/starlight"
python3 scripts/verify-starlight.py "$assets" --ports 30804 30805 \
  --switch-port 30803 --upstream-cases --external-pmp --logs "$logs/starlight-pmp"
python3 scripts/verify-starlight.py "$out/assets" --ports 30807 30808 \
  --switch-port 30806 --recovery --linux-reboot --logs "$logs/starlight-reboot"
python3 scripts/verify-rv64-virt.py "$work" --logs "$logs/virt"
python3 scripts/verify-fire.py "$fire_assets" --max-steps 9000000000 \
  --timeout 1800 --logs "$logs/fire"
```

`cargo test` は全 89 件通過し、上流の Rust 移植 19 テストが含まれます。 StarLight の上流 7 ケースが通過し、両ゲストの root ログイン、通常構成の 56・3200 バイト ping と外部 PMP 構成の 56 バイト ping で各 10/10 応答を確認しました。両構成で両ゲストの `eth0` IRQ が増加しました。RV64 virt は OpenSBI v1.6 から Linux 5.10.6 を起動し、UART ログイン後の `id -u` が `0`、`uname -m` が `riscv64` を返しました。

標準 `reboot` は、GPIO 再起動ドライバーのバインドを確認してから実行しました。`Restarting system`、2 回目の OpenSBI、UART 再認証を確認し、再起動後の 56・3200 バイト ping は両方向で各 10/10 応答でした。公開 FIT に同梱のカーネルには GPIO 再起動ドライバーがないため、このケースでは前述の手順で作成した FIT を使います。 Fire の SD 起動ケースも再実行し、UART ログイン後に読んだ先頭セクターの SHA-256 が公開 SD イメージと一致しました。

#### StarLight の外部 PMP ケース

REPL の `Miscellaneous.RiscVExternalPMP @ CPU` で対象ハートに外部 PMP を接続します。Renode と同様に有効な領域がない間はアクセスを許可し、領域を設定すると PMP の一致判定と権限を適用します。検証スクリプトは上流 Robot と同じ 2 つの宣言を一時 REPL に追加し、公開イメージを起動します。[Renode RiscVExternalPMP](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Cores/RiscV/RiscVExternalPMP.cs)

```sh
cargo build --release
python3 scripts/verify-starlight.py "$assets" --ports 30667 30668 \
  --switch-port 30666 --upstream-cases --external-pmp
```

外部 PMP の上流対応 3 ケースが通過しました。両ゲストで root ログインと `id -u`／`uname -m` を確認し、56 バイトの ping は 10/10 応答でした。`eth0` IRQ は A が 3 → 14、B が 2 → 13 でした。

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

# 上流スクリプトの構成と HSS の開始を確認する。
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

上流の `SdCardFromFile $image mmc 0x100000000` は、ファイルを裏付けにした公称容量 4 GiB の SD カードを `mmc` に接続します。永続化は既定で有効です。`mmc` は `0x20008000` にマップされる MPFS SD コントローラーで、PLIC の IRQ 88 と WakeupIRQ 89 に接続されます。Penv の実装範囲と残課題は下の SD イメージ接続の節に記録しています。[Renode SDCardExtensions](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SD/SDCardExtensions.cs) [Renode MPFS_SDController](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SD/MPFS_SDController.cs)

今回の `cargo test` は全 64 件が通過しました。CLI テスト `polarfire_sysreg_handles_hss_access_and_register_defaults` は HSS と同じオフセットへの書き込みと、PLL 状態およびクロック・ソフトリセットレジスターの読み戻しを確認します。

前回は上記の StarLight 公開素材を使った `python3 scripts/verify-starlight.py "$assets"` でも、UART から root にログインし、`id -u` が `0`、`uname -m` が `riscv64` を返しました。

#### Fire の L2 キャッシュと DDR トレーニング

上流の `SilenceRange` に含まれる PolarFire L2 way-mask レジスターに限り、HSS の `amoswap.d` を受け付けて 0 を返します。続く `mss_nwc_init` では静的なタグだけだとトレーニングが進まないため、上流 REPL の `MPFS_DDRMock` を移植しました。32 ビットの固定応答と交互応答、8 ビットのレーン選択状態はインスタンスごとに管理します。[Renode MPFS_DDRMock](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Miscellaneous/MPFS_DDRMock.cs)

前節の直接起動で `--max-steps 4885985` を指定すると、e51 は `0x20233214` まで実行します。次のステップの `0x20110008` 読み出しはロードアクセス例外となり、`0x20220350` に入ります。対象は最初の Cadence GEM のネットワーク状態レジスターです。この段階では UART はまだ空です。`cargo test` は 65 件通過しました。

#### Fire の HSS バナーまでの起動

Cadence GEM の Network Status は、両方のバスエイリアスで管理インターフェイスの idle ビットを返し、書き込みを無視します。さらに HSS の USB 初期化がアクセスする未定義の 32 ビットオフセット `0x3fc` は上流と同じく 0 を返し、書き込みを破棄します。GEM の送受信と USB 転送は未実装です。[Renode CadenceGEM](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Network/CadenceGEM.cs) [Renode MPFS_USB](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/USB/MPFS_USB.cs)

前節の直接起動を `--max-steps 150000000` で実行すると、UART に HSS の展開成功、バージョン `0.99.36-BVF-0.3.0-2-g3d347fe`、`Attempting to select eMMC` が出ます。その後、e51 の `pc=0x0a0144c8` でファブリックのメディア選択 `0x4fffff00` への書き込みがストアアクセス例外になります。バナー内の OpenSBI 1.2 は HSS に組み込まれたバージョン表示です。ゲストの起動は未到達です。

#### Fire の SD イメージ接続

`SdCardFromFile $image mmc 0x100000000` を MPFS SD コントローラーへの読み出し専用の接続として実装しました。SDHC/SDXC の容量検証、Present State の挿入状態、CMD0・CMD8・CMD55/ACMD41・CMD2/3/7/9/13/16 と単一セクターの CMD17 PIO 読み出し、完了ステータスの W1C を実装しています。容量末尾までのファイルに含まれない領域は 0 を返します。主 IRQ は REPL の PLIC 入力に接続します。書き込みと永続化、ADMA、WakeupIRQ は残課題です。旧 MMC 経路と SDMA の検証は後述します。

```sh
PENV_FIRE_SD_IMAGE="$(find "$fire_assets" -maxdepth 1 -name 'sdcard.img-s_*')" \
  cargo test --test cli polarfire_resc_reads_a_sector_from_an_attached_sd_image
```

ゲストのプローブがファブリック選択レジスター `0x4fffff00` に 1 を書くと SD カードが見えるようになります。リセット直後は eMMC 側です。Renode の `SdCardFromFile` と同じく、接続先はコントローラーが持つ単一の媒体です。プローブにより OCR、4 GiB の CSD、CMD17 の先頭 512 バイトとステータスクリアを確認しました。上流 RESC は `SdCardFromFile` を解析して次の未対応コマンド `SetGlobalQuantum` で停止します。[HSS のメディア選択](https://github.com/polarfire-soc/hart-software-services/blob/5e3f86966381ee9acb94969d0e18ad666f272a30/baremetal/polarfire-soc-bare-metal-library/src/platform/mpfs_hal/common/nwc/mss_io.c)

Fire の直接起動を `--max-steps 400000000` で実行すると、MPFS SD ホストの初期設定を通過します。eMMC 未接続のため選択は失敗し、HSS は CLI の待機を経て `Initializing Mi-V IHC` まで進みます。次に `pc=0x0a01353e` からの `0x50000004` 書き込みでストアアクセス例外になります。この時点では Mi-V IHC、GPIO ボタンの配線、残りの SD コマンドと DMA、RESC のシンボルロードと CPU フックが残っています。Fire の U-Boot と Buildroot ログインは未到達です。[Fire の IHC アドレス定義](https://github.com/polarfire-soc/hart-software-services/blob/5e3f86966381ee9acb94969d0e18ad666f272a30/boards/mpfs-beaglev-fire/drivers_config/fpga_ip/miv_ihc/miv_ihc_add_mapping.h)

継続作業では `cargo test` 68 件が通過しました。StarLight の上流対応 4 ケースを公開ファームウェアで再実行し、両ゲストの root ログイン、56 バイトと 3200 バイトの ping 各 10/10 応答、`eth0` IRQ の A 側 3 → 61 と B 側 3 → 60 を確認しました。外部 PMP の結果はこの次の節に記録しています。RV64 virt は OpenSBI v1.6、Linux 5.10.6、UART ログインを経て `id -u` が `0`、`uname -m` が `riscv64` を返しました。

#### Fire の Mi-V IHC と MMC 起動試行

Fire の 5 ハート用 Mi-V IHC に、ハート間のメッセージレジスター、受信確認、アグリゲーターの割り込みマスクと PLIC 接続を追加しました。Rust のテストで片方向のメッセージ転送と受信確認の IRQ を確認しています。[Mi-V IHC のレジスター定義](https://github.com/polarfire-soc/hart-software-services/blob/5e3f86966381ee9acb94969d0e18ad666f272a30/baremetal/drivers/fpga_ip/miv_ihc/miv_ihc_regs.h)

Fire の直接起動を `--max-steps 410000000` で再実行し、実 HSS が `Initializing Mi-V IHC`、`Initializing IPI Queues`、`Initializing Boot Image`、`Trying to boot via MMC` を出力することを確認しました。この実行は命令上限で停止しました。GPIO ボタン、SD/eMMC 選択から実際の起動イメージのロード、後続の RESC コマンドが残っています。`cargo test` は 69 件通過しました。

#### 公開 SD から Fire の HSS ペイロードを読み込む

公開 HSS は接続したカードに旧 MMC のコマンド列を送ります。この経路の CMD1、CMD6、EXT_CSD、CMD21 のチューニングブロック、CMD12 と、CMD17/18 の読み出し専用 SDMA を実装しました。SDMA は 64 ビットの宛先と複数ブロックを扱い、転送結果をコントローラーのステータスと PLIC に反映します。[Renode MPFS_SDController](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SD/MPFS_SDController.cs)

```sh
# 上流 RESC の素材、プラットフォーム、カード接続と ELF ロードを使う。
fire_probe=$(mktemp)
sed -n -e '/^\$/p' -e '/^using sysbus$/p' -e '/^mach create$/p' \
  -e '/^machine LoadPlatformDescription /p' -e '/^machine SdCardFromFile /p' \
  -e 's/^    \(sysbus LoadELF .*\)$/\1/p' "$fire_resc" > "$fire_probe"
target/release/penv run --script "$fire_probe" --platform-root ../renode \
  --asset-dir "$fire_assets" --uart-device mmuart0 --max-steps 500000000 --stats
```

公開 HSS がカードを初期化し、GPT ヘッダーとパーティション表を検証しました。起動パーティションから 701,624 バイトを DDR の `0x103fc00000` に読み込み、`Boot image passed CRC` と `Boot image set name: "PolarFire-SoC-HSS::U-Boot"` を出力しました。U-Boot 本体の起動と Linux は未到達です。ユニットテストは SDMA の複数ブロック転送と未マップ宛先のエラーも検証します。

#### Fire の OpenSBI と U-Boot 起動

HSS がペイロード登録後に `0x2000100c` を読み出すため、PolarFire のウォッチドッグの設定・ステータス、リフレッシュウィンドウ、IRQ とリセット要求を実装しました。前節と同じ公開素材とコマンドで、実行中の OpenSBI v1.2、U-Boot 2023.07.02、`Model: BeagleBoard BeagleV-Fire` を UART で確認しました。U-Boot は MMC 列挙中の `0x200082fe` の 16 ビット読み出しでロードアクセス例外になります。[Renode MPFS_Watchdog](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Timers/MPFS_Watchdog.cs)

#### Fire の U-Boot MMC 列挙

MPFS SD ホストに 8・16・32 ビットのレジスターアクセス、ホストクロックとソフトリセットを追加しました。公開イメージで前節の手順を `--max-steps 650000000` まで実行し、U-Boot の `MMC: mmc@20008000: 0` とシリアル入出力の初期化を確認しました。次は `Net:` に続く Cadence GEM の `0x20110294` への読み出しでロードアクセス例外になります。これは Design Configuration 6 です。Linux 起動は未到達です。

#### Fire の U-Boot から SD のブートスクリプトを実行

Cadence GEM の設計情報、ステーションアドレスと MDIO を実装し、U-Boot のネットワーク列挙を通過しました。MPFS SD は CMD0 後にネイティブ SD と旧 MMC を再識別し、SD の SCR（ACMD51）とカード状態（ACMD13）を PIO または SDMA で転送できます。複数ブロックの PIO 読み出しも追加しました。Cadence GEM のフレーム DMA と実データの通信は未実装です。

公開イメージで前節の手順を `--max-steps 1250000000` まで実行し、`Net: eth0: ethernet@20110000`、`mmc0 is current device`、`Scanning mmc 0:2...`、`Found U-Boot script /boot.scr` とスクリプト実行開始を確認しました。[U-Boot の MMC モード選択](https://github.com/u-boot/u-boot/blob/v2023.07/drivers/mmc/mmc.c) [Renode CadenceGEM](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Network/CadenceGEM.cs)

#### Fire の Linux コンソールと残課題

```sh
cargo test
cargo build --release
python3 scripts/verify-fire.py "$fire_assets" --until linux
# ログインまで追う場合は --until linux を外す。
```

公開 HSS と SD イメージの実行を `--max-steps 6000000000` の上限まで追いました。U-Boot は SD 上の FIT を読み込み、カーネルと DTB の SHA-256 を検証しました。`Starting kernel` の後に Linux `6.6.35-linux4microchip+fpga-2024.06` が起動し、`20000000.serial` を `ttyS0` コンソールとして有効にしました。カーネル引数は元のイメージの `root=/dev/mmcblk0p3 rootwait` を使用しています。

前回の最初の阻害点は `mchp_corespi_probe+0x174` の 32 ビット読み出しです。Linux は仮想アドレス `0xffff8f800004d000` でロードアクセス例外を報告し、起動中にパニックになりました。この阻害点の修正と次の例外は下の CoreSPI の節に記録しています。[PolarFire SoC の REPL](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/polarfire-soc.repl)

この時点の検証スクリプトは上流 RESC の一部を抽出していました。元の RESC 全体への対応は後述します。この段階では `cargo test` 75 件、StarLight の上流対応 7 ケース、RV64 virt の実 Linux ログインが通過しました。

#### Fire の CoreSPI プローブ

Fire の FIT に含まれる DTB は `spi@20108000` と `spi@20109000` を有効にしています。Penv は上流 REPL の `SPI.MPFS_SPI` を認識し、それぞれの 4 KiB の窓に CoreSPI の 32 ビット制御・状態レジスターを割り当てます。Linux のプローブで使う制御、フレーム幅、クロック分周、チップ選択とリセットを実装しました。SPI スレーブ、データ転送、転送割り込みは残課題です。[Renode MPFS_SPI](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SPI/MPFS_SPI.cs) [Linux 6.6 CoreSPI ドライバー](https://github.com/torvalds/linux/blob/v6.6/drivers/spi/spi-microchip-core.c)

```sh
cargo test
cargo build --release
python3 scripts/verify-fire.py "$fire_assets" --max-steps 6500000000 \
  --logs target/fire-spi-probe
```

比較には同じ公開 HSS と SD イメージを使いました。FIT 内の元の DTB を使い、U-Boot のハッシュ検証が通過しました。修正前の `17b85ea` は Linux 6.6.35 の `console [ttyS0] enabled` の後、`mchp_corespi_probe+0x174` のロードアクセス例外を再現しました。修正後は同じコンソールの初期化を経て、`20108000.spi` と `20109000.spi` がそれぞれコントローラー 0 と 1 を登録しました。

この段階の次の停止は `mpfs_musb_set_vbus` が呼ぶ `musb_default_readb` の USB 対象オフセット `0x60` への 8 ビット読み出しです。検証スクリプトはログイン待ちでこのカーネルパニックを検出します。SD ルートのマウントからログインまでの検証は次節に記録しています。この段階の `cargo test` は 76 件通過しました。

#### Fire の上流 RESC と Linux 周辺機器のプローブ

`verify-fire.py` は上記の公開素材と上流の `beaglev-fire.resc` をそのまま使います。Fire の設定に必要な GPIO ボタン操作と、ELF の `_start` で管理ハートを停止する CPU フックを実装しました。`SetGlobalQuantum` は時間指定の妥当性を検査します。Penv は命令単位でハートを切り替えるため、スケジューリングの粒度は変わりません。フックは上流スクリプトの停止処理に対応します。任意の Python フックの実行と管理ハート停止時の Monitor メッセージは未対応です。

CoreSPI の次に発生した MUSB のプローブ例外は、接続機器がない場合の USB レジスターと 8 ビット DEVCTL の実装で解消しました。その次の CoreI2C には初期状態、スレーブ不在時の NACK と PLIC 割り込みを実装しました。CoreQSPI にはプローブ時のレジスター応答を実装しました。USB の転送、I2C スレーブ、SPI/QSPI のデータ転送と GPIO 割り込みは残課題です。[Renode MPFS_USB](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/USB/MPFS_USB.cs) [Renode MPFS_I2C](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/I2C/MPFS_I2C.cs) [Renode MPFS_QSPI](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/SPI/MPFS_QSPI.cs)

```sh
cargo test
cargo build --release
python3 scripts/verify-fire.py "$fire_assets" --max-steps 9000000000 \
  --timeout 1800 --logs target/fire-sd
```

検証スクリプトは UART から root としてログインし、`uname -m` と `uname -n` を確認します。続いてゲストの `/dev/mmcblk0` の先頭 512 バイトを `dd` で読み、ホスト側の公開 SD イメージの同じ範囲と SHA-256 が一致した場合にのみ PASS を出力します。UART と標準エラーは指定ログディレクトリに残ります。

公開イメージで `PASS Should Boot Buildroot From SD Card` を確認しました。U-Boot で元の FIT 内のカーネルと DTB の SHA-256 検証が通過し、Linux は両 CoreSPI、両 CoreI2C と SDHCI を初期化しました。DMA で GPT を読み出して 3 パーティションを認識し、`mmcblk0p3` の ext4 をマウントしました。UART ログイン後の `uname -m` は `riscv64`、`uname -n` は `buildroot` を返しました。ゲストとホストの先頭セクターの SHA-256 はともに `c0d1a9055fedfc78ee08193dc4b6c269c1124988d6727d62d02649595b016912` でした。`cargo test` は 83 件通過しました。

StarLight の外部 PMP 付き上流 3 ケースも前掲のコマンドで再実行しました。両ゲストの UART root ログインと 56 バイト ping の 10/10 応答を確認し、両側の `eth0` IRQ は 3 → 14 でした。

SD の書き込みが未実装なので ext4 の書き込み可能な再マウントは失敗し、ルートは読み取り専用で動作します。Fire の Ethernet は PHY 接続とフレーム転送が残っています。起動ログには GNU Parted の GPT 末尾位置の警告と、非コヒーレント DMA に関する Linux の警告も残ります。

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

### nRF52840 の上流 Zephyr Shell

[従来の受け入れ対応表](tests/nrf52840-acceptance.md) · [固定上流の全件台帳と追加検証](tests/nrf52840-full-upstream.md)

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

## STM32F407 Discovery と FreeRTOS

`penv/stm32f407` は `3686ab4` を起点に、Cortex-M4 の整数命令を既存の
Thumb-2 実装で実行します。NVIC、SysTick、SVC、PendSV、PSP/MSP の処理も共用します。
`UART.STM32_UART` は F4 の SR/DR/CR1 レイアウトを使用し、既存の STM32 USART の
入出力と割り込み配送を共用します。

上流の `stm32f4_discovery.resc` は Contiki 用です。未変更の RESC での起動手順は下記にあります。
未変更の上流ボード定義で FreeRTOS を起動する手順は下記の「未変更の上流 REPL」にあります。
FreeRTOS 検証用の REPL には、上流を参照して UART4（`0x40004c00`、IRQ 52）、
TIM2（`0x40000000`、IRQ 28）、GPIOC、NVIC の配線を含めました。メモリー容量は STM32F407VG の 1 MiB フラッシュと
128 KiB メイン SRAM を使用します。リセット時は HSI 16 MHz で、ファームウェアは
RCC から SysTick を含む HCLK を 32 MHz に切り替えます。
ファームウェアは PC10/PC11 の AF8 を設定し、UART4 から 115200 baud 設定で送信します。
[`stm32f4_discovery.resc`](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/scripts/single-node/stm32f4_discovery.resc)
[`stm32f4.repl`](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/platforms/cpus/stm32f4.repl)
[STM32F407VG データシート](https://www.st.com/resource/en/datasheet/stm32f407vg.pdf)

最初の TIM2 割り込みでは、FreeRTOS の `xQueueSendFromISR` が IRQ 優先度を
検証するために SCB_AIRCR（`0xe000ed0c`）を読み、未マップアクセスで停止しました。
NVIC モデルはリセット値 `0xfa050000` を返し、キー付きの書き込みで `PRIGROUP` を変更できます。
[STM32 Cortex-M4 プログラミングマニュアル PM0214、4.4.5](https://www.st.com/resource/en/programming_manual/pm0214-stm32-cortexm4-mcus-and-mpus-programming-manual-stmicroelectronics.pdf)

### ビルドと実行

Git、Clang、LLD、Rust/Cargo が必要です。下記は Linux x86_64 上で Clang/LLD 15.0.7 と
ホストリンカー GCC を使った実行例です。ビルドスクリプトは公式 FreeRTOS-Kernel
V11.1.0（`dbf70559b27d39c1fdb68dfb9a32140b6a6777a0`）を取得し、公式
`portable/GCC/ARM_CM3` の整数ポートを `-mcpu=cortex-m4 -mfloat-abi=soft` で
リンクします。起動コード、メモリー配置、タスクは
`tests/fixtures/stm32f407-freertos/` にあります。
[FreeRTOS-Kernel V11.1.0 の ARM_CM3 ポート](https://github.com/FreeRTOS/FreeRTOS-Kernel/tree/dbf70559b27d39c1fdb68dfb9a32140b6a6777a0/portable/GCC/ARM_CM3)
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

### backlog-network-66b4cd80a9d2-1

GPIO で同一の送信ピン・受信先・入力番号を再接続しても、端点は一つに保たれます。固定上流 `ShouldConnectGPIOToReceiverAndReturnTheSameReceiver` の受信先同一性と、他の送信ピンが空であることを照合しました。`ShouldDisconnectEndpointOfUnregisteredPeripheral` は、複数送信元、重複抑止、対象端点だけの解除を STM32H7 GPIO/TIM で確認しました。上流の中継ノードは送信元と受信先を兼ねるため、後者は部分検証です。固定 STM32H743 の CLI で重複した接続を受理し、TIM3 解除後に TIM2 だけが残ることも確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-rcc-timer-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-gpio-reconnect.Hc85piXC
target/release/penv run --script "$e/reconnect.resc" --platform-root ../renode
```

`cargo test` 220件、STM32H7 REPL 7構成、構文診断3件、RCC/TIM1 3件、公開 Zephyr Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。対象786件のうち等価合格199件、部分検証18件、未達569件です。中継ノードと外部・別マシンの接続解除が残っています。

### backlog-network-bb984cbcaa5f-1

上流 `SDCard.robot` のファイル装着3件を移植しました。固定 H743 REPL の同型 SDMMC で、上流 Robot ファイル自身を容量 `0x1000` で装着できることと、存在しないファイル・空パスの上流診断を確認しました。このプローブでは SDHC の最小容量までメモリー上でゼロ埋めします。元ファイルへの永続化と、アドレスを指定しない単体 SDMMC の構築は残っています。共有変更は ARM 担当へ引き渡しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-platform-errors-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-display-sd-smoke.sh ~/.local/state/penv-dispatch/evidence/h7-board-pass.aBYgFTkr/shell
```

`cargo test` 220件、固定 STM32H7 REPL 7構成、構文診断3件、Zephyr Blinky・Button・表示・SD の実 ELF が通過しました。証拠は `~/.local/state/penv-dispatch/evidence/h7-sd-file.5coRI4Lq` にあります。台帳は等価合格202件、部分検証18件、未達566件です。GPIO の中継ケースには同一ノードの受信・再送信を表す接続モデルが必要です。

### 担当変更後の独立検証

共有の SD ファイル装着は `network-sd-card-03830d3.md` で ARM 担当に引き渡しました。STM32H753ZI では別に長文 UART と GCM の公開 ELF を再現しました。長文 UART は固定手順で2回のプロンプト照合を通過し、追加の0.7仮想秒で `RX ring buffer full` を再現したため部分検証のままです。GCM のポーリング版と割り込み版は、両方の ELF が EABI5 hard-float として読み込み段階で拒否されました。共有の ELF 対応は `network-h7-hardfloat-03830d3.md` で ARM 担当に依頼し、CRYP の GCM 処理と LED 判定は機種固有の残件として台帳に保持しています。

```bash
export PENV_ASSET_DIR=/tmp/penv-h7-assets
bash tests/stm32h7-nucleo-uart-line.sh "$(mktemp -d)"
u=~/.local/state/penv-dispatch/evidence/h7-uart-independent.szDKA2I0
g=~/.local/state/penv-dispatch/evidence/h7-gcm-baseline.zrPeub83
target/release/penv run --script "$u/extended/case.resc" --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 18000000
for test in CRYPTO_GCM CRYPTO_GCM_IT; do
    target/release/penv run --script "$g/$test.resc" --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 3000000
done
```

各証拠ディレクトリにケースのログと終了状態を保存しました。拡張 UART と両 GCM は終了状態1です。

### backlog-network-27146221a294-1

STM32H7 FDCAN のデバイス側に、内部ループバックの古典 CAN フレーム転送を追加しました。メッセージ RAM の TX 要素を RX FIFO0 に複製し、送信完了、受信通知、FIFO 状態、ACK を単体テストで照合しました。固定 Nucleo の公開 CAN ELF は初期化とカウンターフィルターの登録まで進み、最初の `Counter received: 0` を8仮想秒待ったところで停止します。現行の共有バスは FDCAN にメッセージ RAM を渡さないため、接続を ARM 担当に依頼します。Zephyr が設定するフィルターと CAN FD はデバイス側の残件です。SD と hard-float の先行依頼は今回確認時点で応答ファイルがありませんでした。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-fdcan-loopback.2Merc2Mj
target/release/penv run --script "$e/can.resc" --platform-root ../renode --max-steps 10000000
```

`cargo test` 221件、STM32H7 REPL 7構成と公開 Zephyr Blinky・Button ELF が通過しました。CAN の CLI は上記の未達で終了状態1です。ケース別結果、UART、終了状態は上記に保存しました。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

共有配線の依頼 `network-fdcan-417181b.md` を登録後、独立した IIS2MDC の公開 polling・trigger ELF も再検証し、両モードが通過しました。上記の `iis2mdc/` に UART と終了状態があります。再現は `PYTHON=<construct 導入済みの仮想環境>/bin/python bash tests/stm32h7-iis2mdc-robot.sh "$(mktemp -d)"` です。

### backlog-network-b9dac4e9689d-1

STM32H7 FDCAN の古典 CAN ループバックに標準 ID フィルターを追加しました。範囲・二重 ID・マスクの判定、先頭一致、拒否、FIFO0 のフィルター番号と非一致ビットを同じテストバイナリで確認しました。拡張 ID フィルター、FIFO1、TX FIFO の状態処理は残っています。

固定 Nucleo と `MCAN.robot` の公開ループバック ELF は、どちらも初期化とフィルター登録を完了し、最初の `Counter received: 0` で8仮想秒タイムアウトしました。両ケースを未達のままとし、共有メッセージ RAM の配線は `network-fdcan-417181b.md` の依頼で追跡します。共有ソースの変更はありません。独立した SD ファイル装着3件を再実行しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-fdcan-filters.43D5rrYw
for case in can mcan; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --max-steps 10000000
done
```

`cargo test` 223件、STM32H7 REPL 7構成、公開 Zephyr Blinky・Button ELF、SD ファイル装着3件が通過しました。上記にケース別結果、UART ログと終了状態を保存しました。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-503aa8981709-1

FDCAN の TX FIFO とキューで、空き数・読み出し位置・書き込み位置・満杯状態を返します。送信要求の登録範囲、キャンセル後の空きスロット、スロット再利用時の完了フラグを同じバイナリで照合しました。固定 H753 REPL の CLI でも、2スロットの FIFO が空・満杯・1スロット空きになることを確認しました。優先度付き送信の選択と TX イベント FIFO は残っています。

Nucleo と `MCAN.robot` の固定 ELF は両方とも `Counter received: 0` で8仮想秒タイムアウトしました。共有 message RAM の接続は既存依頼 `network-fdcan-417181b.md` で追跡しています。ARM からは、最小構成で hard-float GCM ELF 2本がロードできるとの結果が届きました。統合側の全 H753 構成で残る TIM17 と LED テスターの阻害点は、既存ローカルコミットを添えて `network-h7-gcm-platform-503aa89.md` に引き渡しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-fdcan-tx-fifo.B1Ctk6HX
for case in fifo can mcan; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --max-steps 10000000
done
```

`cargo test` 225件、STM32H7 REPL 7構成、Zephyr Blinky・Button、独立した SD 装着3件が通過しました。ログと終了状態は上記に保存しました。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-947817b5689c-1

STM32H7 FDCAN の送信イベント FIFO を実装しました。固定上流 MCAN のイベント要素に従って ID・DLC・マーカーをメッセージ RAM に記録し、新規登録・ウォーターマーク・満杯・欠落と ACK 後の空状態を検証しました。タイムスタンプは固定上流モデルと同じゼロです。広幅マーカー用の外部タイムスタンプ機構は未対応です。

Nucleo と `MCAN.robot` の公開ループバック ELF を再実行しました。両方とも最初の `Counter received: 0` で8仮想秒タイムアウトしたため、台帳は未達のままです。共有 message RAM の配線は ARM の既存依頼 `network-fdcan-417181b.md` で追跡しています。独立した SD ファイル装着3件も再実行しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-fdcan-tx-events.rH2f7XFi
for case in can mcan; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --max-steps 10000000
done
```

`cargo test` 227件、STM32H7 REPL 7構成、Zephyr Blinky・Button、SD ファイル装着3件が通過しました。上記にケース別結果、UART と終了状態を保存しました。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-eba880b30233-1

FDCAN の共有 message RAM 配線は ARM からの返答待ちのため、独立した STM32H7 CRYP を進めました。AES-GCM の初期化、整列ヘッダー・ペイロード、最終タグをデバイスに実装しました。128ビットの既知ベクトルで、ポーリング暗号化と IRQ 復号のデータ、タグ、FIFO が空になった後の IRQ 解除を同じバイナリで照合しました。AES-192/256 の経路も実装しました。今回の既知ベクトルは AES-128 です。端数ブロックと上流 HAL の CTR 切替ワークアラウンドは残っています。

固定上流 GCM ELF 2本はローカルの共有ローダーで hard-float ABI を拒否され、LED の観測まで進みませんでした。ARM の最小構成ではロード済みで、全 H753 構成への接続は既存の `network-h7-gcm-platform-503aa89.md` で追跡しています。共有ソースの変更はありません。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-gcm-aligned.vaMiF7Ll
for case in CRYPTO_GCM CRYPTO_GCM_IT; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 3000000
done
```

`cargo test` 229件、固定 STM32H7 REPL 7構成、公開 Zephyr ADC DMA・暗号・Blinky・Button ELF が通過しました。ケース別結果と終了状態は上記に保存しました。GCM の Robot 2件は未達です。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-36174e540c9e-1

STM32H7 CRYP の AES-GCM が、最終フェーズで渡されたビット長を使って端数ブロックのタグを計算します。AAD と暗号文で最後のブロックだけを保留し、長さに含まれないパディングを認証から除きます。7バイトの暗号化と、6バイトの AAD および19バイトの復号を同じバイナリで検証しました。期待出力は Python `cryptography` の AESGCM でも照合し、ログに保存しました。上流 HAL の CTR 切替ワークアラウンドは残っています。

固定公開 GCM ELF 2本はローカルの hard-float ローダーで停止し、LED 判定には到達しませんでした。全 H753 構成の共有阻害点は既存の ARM 依頼 `network-h7-gcm-platform-503aa89.md` で追跡しています。別経路として ADC DMA、既存暗号サンプル、SD ファイル装着を再実行しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-gcm-partial.ricAgiKH
for case in CRYPTO_GCM CRYPTO_GCM_IT; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 3000000
done
```

`cargo test` 231件、固定 STM32H7 REPL 7構成、Zephyr ADC DMA・暗号・Blinky・Button ELF、SD 装着3件が通過しました。ケース別結果、公開 ELF の停止ログと終了状態は上記にあります。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-aee087314a38-1

STM32H7 CRYP が AES-GCM のペイロード処理中に HAL の CTR 切替を受けても、認証状態を保持します。固定上流モデルと同じく、最初の Final ブロックに対して HAL が読み捨てる4ワードを返し、次の長さブロックで有効なタグを返します。7バイトのポーリング暗号化と IRQ 復号を同じバイナリで照合しました。

固定公開 GCM ELF 2本はローカルでは hard-float ABI の読み込みエラーで停止しました。全 H753 構成の共有阻害点は既存の ARM 依頼 `network-h7-gcm-platform-503aa89.md` で追跡しており、LED 判定は未達です。独立した ADC DMA と SD ファイル装着を回帰確認しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-ztest-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-gcm-ctr.S0mDzHKq
for case in CRYPTO_GCM CRYPTO_GCM_IT; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --asset-dir "$PENV_ASSET_DIR" --max-steps 3000000
done
```

`cargo test` 233件、固定 STM32H7 REPL 7構成、Zephyr ADC DMA・暗号・Blinky・Button ELF と SD 装着3件が通過しました。ケース別結果と終了状態は上記に保存しました。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-8e79fd74efc6-1

STM32H7 FDCAN の FIFO0 に拡張 ID フィルターを実装しました。固定 MCAN と同じ2ワードのフィルター形式、グローバルマスク有無の範囲、二重 ID、クラシックマスクと最初の一致を、同じバイナリの3テストで照合しました。Nucleo と `MCAN.robot` の公開 ELF は最初のカウンターで8仮想秒タイムアウトしました。共有 message RAM 配線は ARM の既存依頼 `network-fdcan-417181b.md` で追跡しています。

配線待ちと独立に、固定 Nucleo の IS25WP 公開 ELF と上流のフラッシュ追記を実行しました。単一セクター試験の開始まで到達し、単一・複数セクターとも消去テスト中に `Flash read failed! -5` を出力しました。読み取りコマンドとステータスの追跡が残っています。共有ソースの変更はありません。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
e=~/.local/state/penv-dispatch/evidence/h7-fdcan-extended.dY4lsY7x
for case in can mcan is25wp; do
    target/release/penv run --script "$e/$case.resc" --platform-root ../renode --max-steps 16000000
done
```

`cargo test` 236件、固定 STM32H7 REPL 7構成、Zephyr Blinky・Button、SD ファイル装着3件が通過しました。公開 CAN 2件と IS25WP は未達です。ケース別結果、UART ログ、終了状態は上記に保存しました。台帳は対象786件のうち等価合格202件、部分検証18件、未達566件です。

### backlog-network-2e61e3575da9-1

STM32H7 QuadSPI の ISSI IS25WP で、間接読み取り、書き込み許可付きページプログラム、セクター消去を実装しました。最初の修正で実 ELF は消去確認まで進み、ページ書き込みで停止しました。レジスタートレースで HAL の同一 CCR 再書き込みが転送を中断することを確認し、転送状態を保持するよう修正しました。固定上流 `Should Read and Write IS25WP Flash` の同じ ELF、REPL 追記、8仮想秒の期限、UART 7行を使い、単一・複数セクターの消去と書き戻し照合を通過しました。

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export PENV_ASSET_DIR=/tmp/penv-h7-assets
cargo test
cargo build --release
bash tests/stm32h7-is25wp-robot.sh "$(mktemp -d)"
bash tests/stm32h7-platform-load.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
bash tests/stm32h7-sd-file-robot.sh "$(mktemp -d)"
```

`cargo test` 238件、STM32H7 REPL 7構成、Zephyr Blinky・Button、独立した SD 装着3件も通過しました。ケース別結果、初回失敗とレジスタートレース、最終 UART ログは `~/.local/state/penv-dispatch/evidence/h7-is25wp-data.B7xtH7T6` に保存しました。台帳は対象786件のうち等価合格203件、部分検証18件、未達565件です。メモリーマップ読み取りと XIP、Macronix フラッシュの上流ケースは残っています。共有ソースの変更はありません。

### H753ZI Macronix QuadSPI

固定上流の Macronix MX25R を H7 QuadSPI に接続し、256 MiB 構成の JEDEC ID `c2 28 22`、間接 Quad Program (`0x38`)、完了 IRQ、間接読み戻しを単体テストで照合しました。固定公開 ELF 3個はファイル名の SHA-1 と照合済みです。

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
RCC PLL 32 MHz, RTC and TIM9 OK
TIM9 FAULTMASK OK
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
- `cargo test` はユニットテスト 65 件、CLI テスト 5 件が通過しました。既存の
  `tests/stm32h7-zephyr-smoke.sh` も通過し、USART2 で `Zephyr version 4.4.99`
  を確認しました。
- 別々の出力ディレクトリでビルドした ELF はバイト単位で一致しました。この環境での
  SHA-256 は `93f6e7b142181ccf75446ebdc9f5274a98d7a9927638c9a944a4200f54c28661` です。

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
キュー、UART4 の出力を全行照合します。PWR の MMIO は RCC の PWREN に従います。
PWR の個別リセット、バックアップ SRAM への DBP の伝播、
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

SPI は転送を同期完了として扱います。SCK/NSS とビット時間、
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

SCL/SDA の信号とタイミング、10 ビットアドレス、スレーブ動作、
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

IWDG の更新待ち状態、フラッシュオプションによる自動起動、デバッグ時停止、ボード全域のリセットは残課題です。

### STM32F407 の RCC

上流の `Miscellaneous.STM32F4_RCC` を `0x40023800` に登録し、`rtcPeripheral` と
`Timer9Reset -> timer9@255` を既存の RTC/TIM モデルに接続します。RCC は HSI/HSE と
主 PLL の選択・レディ状態、AHB/APB 分周、主要周辺機器のクロックゲートを扱います。
SysTick は HCLK、TIM は対応する APB のタイマー周波数に追従します。RTC は BDCR で
選んだクロックを使用し、BDRST でバックアップ領域をリセットします。
[Renode の STM32F4_RCC モデル](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/Miscellaneous/STM32F4_RCC.cs)
[RM0090 の RCC 章](https://www.st.com/resource/en/reference_manual/dm00031020-stm32f407-417-427-437-4x9-4a9-4x7-advanced-arm-based-mcus-stmicroelectronics.pdf)

スモークテストの ELF は HSE 8 MHz から PLL で HCLK 32 MHz を生成し、APB1/APB2 を
16 MHz に設定します。SPI1 のゲート、TIM9 のリセット、DBP による BDCR 保護、
LSE による RTC の有効化とバックアップレジスターの消去を確認して
`RCC PLL 32 MHz, RTC and TIM9 OK` を出力します。IWDG 再起動時には HSI とクロックゲートの
初期状態、`RCC_CSR.IWDGRSTF`、`RMVF` によるフラグ消去を確認してから同じ設定を実行します。

発振器の起動時間、PLL の電気的制約、IWDG による LSI の強制有効化、補助 PLL の出力クロック、RCC IRQ、
低電力モードのクロック、TIM9 以外の個別周辺機器リセットは残課題です。

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

上流の `stm32f4_discovery-kit.repl` をそのまま読み込み、FreeRTOS の通常起動と
IWDG 後の再起動を実行できます。タイマーと GPIO の追加宣言を既存定義に適用し、TIM1/TIM8 の
分割 IRQ、GPIO の代替機能・EXTI・LED の配線、ボタンの初期状態、2 つのビットバンド領域を
扱います。CRC と RNG の F4 指定を受理します。複数 UART がある構成では
`showAnalyzer sysbus.uart4` が UART4 を入力先に選びます。
cargo build --release
export PENV_ASSET_DIR=/tmp/penv-h7-assets
bash tests/stm32h7-mx25r-robot.sh "$(mktemp -d)"
bash tests/stm32h7-is25wp-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-led-robot.sh "$(mktemp -d)"
```

`cargo test` 239件、IS25WP の UART 7観測、Blinky・Button が通過しました。Macronix の間接転送 ELF は `Mocks.DummyI2CSlave` の登録で停止し、MemoryMapped と XIP は hard-float ELF の読み込みで停止しました。次の検証には間接転送とバス窓のデータ共有、XIP の最終 PC `0x90000010` の照合が残ります。再現用 REPL/RESC、SHA-1、ケース別ログと終了状態は `~/.local/state/penv-dispatch/evidence/h7-mx25r.Md9KKNw9` に保存しました。台帳は対象786件のうち等価合格203件、部分検証18件、未達565件です。

### H753ZI Ethernet PTP

固定上流 Zephyr PTP ELF の起動は、変更前に UART 無出力で停止しました。H7 MAC にタイムスタンプ初期化・更新レジスターと仮想経過時間を受ける時計を実装すると、同じ上流 REPL で Port 1 の初期化、シェルプロンプト、`ptp_clock set PTP_CLOCK 25` 後の `25.00` を確認できました。現状は共有経路から経過時間が渡らず、7.48 仮想秒後の読み出しも `25.000000000` でした。デバイス単体では同じ経過時間で `32.48` に到達します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/stm32f407-freertos-smoke.sh target/stm32f407-freertos
tests/stm32f407-upstream-smoke.sh target/stm32f407-upstream
```

`RENODE_ROOT` の既定値は `../renode` です。上流スモークテストは公開 ELF の SHA-1 を確認し、
`PENV_ASSET_DIR` を指定した場合はそのディレクトリをキャッシュとして使用します。
FreeRTOS の上流構成では、基板上にエコー機器がない状態で SPI のゼロ応答と I2C の NACK を
確認します。UART 出力には `Discovery bit-band GPIO and SRAM OK` と
`I2C unpopulated bus NACK OK` が現れ、残りの TIM2、キュー、CAN、UART、IWDG の期待出力は
検証用 REPL と共通です。上流構成の ELF の SHA-256 は
`01a5823906138f045a7545050cff3051023e98bb5f6f78f67157e8cb4056283a` です。

Renode の TIM2 上り・下りカウントケースは公開 ELF のまま移植し、開始後の
`period elapsed callback` をそれぞれ 3 回以上確認します。Zephyr Hello World の公開 ELF は
未変更の kit REPL から起動し、`Hello World! stm32f4_disco` を確認します。
FAULTMASK の 2 ケースは公開 ELF の TIM3 と FreeRTOS ELF の TIM9 で実行します。
フラッシュの消去モード未指定、一括消去、セクター 11 の選択消去は Rust テストで照合します。
[上流 STM32F4 Discovery テスト](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/STM32F4_Discovery.robot)
[上流フラッシュ消去テスト](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/stm32f4-erase.robot)

### 上流の PWM キャプチャと直交位相デコーダー

上記の `tests/stm32f407-upstream-smoke.sh` は、公開 ELF を使用した PWM キャプチャの
通常版・4 チャネル版と、直交位相デコーダーも実行します。テスト用 REPL は未変更の
CPU REPL を読み込み、Robot テストと同じ配線を追加します。PWM は PA6 → PD12、
直交位相は PD12 → PA6 と PD13 → PA7 です。FreeRTOS 回帰には未変更の kit REPL を使います。
[上流 STM32F4 Discovery テスト](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/STM32F4_Discovery.robot)

PWM は初回の周波数行を読み飛ばし、2 回目の周波数とデューティ比の整数部を
Robot と同じ許容範囲（1000 Hz ±25 Hz、33% ±1 ポイント）で照合します。
直交位相の最初の 2 出力は 10°・20°（各 ±1°）です。実行ログは出力先の
`pwm-capture.log`、`pwm-capture-four-channel-capture-support.log`、`qdec.log` です。

タイマーモデルは直接・間接キャプチャ、TI1/TI2 トリガーでのカウンターリセット、
キャプチャフラグとオーバーキャプチャ、エンコーダーモードの方向とカウンター境界を
扱います。GPIO 間の配線は出力レベルを入力側に伝え、選択された代替機能へ渡します。
公開 ELF の初期化に必要な PLLI2S の設定・レディ状態、DBGMCU_CR、ADC 共通の
クロック分周設定にも対応しました。
[RM0090 の STM32F407 タイマー章](https://www.st.com/resource/en/reference_manual/dm00031020-stm32f405-407-415-417-437-455-469-application-note-stmicroelectronics.pdf)

4 チャネル版は最初のエッジを受けた ISR でカウンターをリセットします。
PWM では固定 Renode の既定 CPU 設定に合わせて `--virtual-mips 100` を指定し、
`--f4-repl-timing` で REPL の TIMx 周波数と SysTick 周波数を保持します。
FreeRTOS 回帰は RCC のゲートと周波数設定を使う既定モードで実行します。
[Renode Infrastructure BaseCPU](https://github.com/renode/renode-infrastructure/blob/09ac7c563095c5410258ecc1b1a6b841a013db85/src/Emulator/Peripherals/Peripherals/CPU/BaseCPU.cs#L384)

残課題は入力のデジタルフィルター、GPIO の多段伝搬・競合、ADC 変換、PLLI2S の出力クロック、
デバッグ時の停止動作、`ApplySVD` の汎用レジスター展開、
AIRCR のソフトウェアリセット要求、実機確認です。
フラッシュプログラム動作、UART のビット時間、CAN の通常モードでのノード間通信も
残課題です。

### Contiki の上流 RESC と Cortex-M4F FreeRTOS

上流スモークテストは公開 Contiki ELF を SHA-1 で照合し、未変更の
`scripts/single-node/stm32f4_discovery.resc` と kit REPL から実行します。
最初の停止原因だった `cpu PerformanceInMips 125` を仮想時間に反映し、
リセットマクロの UID 書き込みには再現可能な疑似乱数を使用します。
ファームウェアの初期化には TIM7 の半ワードアクセス、SYSCFG の GPIO–EXTI 選択と
MII/RMII 選択レジスターが必要でした。`contiki.log` の UART4 出力で
`Contiki 3.x started.`、`ETH_BSP_Config done!`、`UDP server started` を確認します。
[上流 Contiki RESC](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/scripts/single-node/stm32f4_discovery.resc)

同じテストは FreeRTOS-Kernel V11.1.0 の公式 ARM_CM4F ポートを
`-mfloat-abi=hard -mfpu=fpv4-sp-d16` でビルドし、未変更の kit REPL で実行します。
2 タスクは別々の S0、S31、FPSCR の値を保持し、`vTaskDelay` 後に照合します。
`cm4f/uart.log` には `FPU context OK: A` と `FPU context OK: B` が現れます。
TIM2 の IRQ とキュー、CAN ループバック、UART エコー、IWDG 再起動後の出力も照合します。
[公式 FreeRTOS ARM_CM4F ポート](https://github.com/FreeRTOS/FreeRTOS-Kernel/tree/dbf70559b27d39c1fdb68dfb9a32140b6a6777a0/portable/GCC/ARM_CM4F)

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/stm32f407-upstream-smoke.sh target/close-f407-platform
# ARM_CM4F のみをビルド・実行する場合
tests/stm32f407-freertos-smoke.sh target/stm32f407-cm4f --fpu
```

残課題は FPv4-SP の残りの算術命令、FPCCR の遅延スタック保存（現状は例外進入時に保存）、
特権限定の FPU アクセス、一般の Python モニターコマンド、
Contiki の DHCP リース取得と UDP 疎通の検証です。

### STM32F407 上流ケースの棚卸し

Renode の参照リビジョンは `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` です。

| 対象 | 上流ケースと penv での検査 |
|---|---|
| PWM・直交位相 | Discovery の `PWM Capture` 2 件と `Quadrature Decoder` を公開 ELF と同じ GPIO 接続で実行。2 番目の周波数とデューティ比、最初の 2 つの角度を照合 |
| タイマー IRQ・FAULTMASK | Discovery の TIM2 上り・下りと FAULTMASK 2 件を公開 ELF で実行。FAULTMASK ELF は上流 REPL と同じ TIM3 クロックで実行し、`Sleeping` 後に仮想時間で 1 秒間 UART が無出力であること、解除後 3 回以上、例外復帰後 4 回以上の割り込みを照合。FreeRTOS では TIM9 と TIM2 のキュー配送も検査 |
| RTC | Discovery の `Should Not Crash On RTC Writes` を公開 Zephyr ELF で実行し、初期値、月変更後、時変更後の UART 出力を順に照合。`Configure RTC Alarm` は公開 RIOT ELF に上流と同じ 32 kHz オーバーレイと UART 入力 `s` を使い、2 秒間隔で計 4 回の `Alarm!` とその後 3 秒間の無出力を検査 |
| I²C | Discovery に専用の上流 ELF ケースは見当たりませんでした。FreeRTOS の検証用 REPL で `01 23` の書き込み・読み戻しとイベント IRQ、未接続アドレスの NACK とエラー IRQ を検査。未変更 kit でも NACK を確認 |
| SPI | Discovery に専用の上流 ELF ケースは見当たりませんでした。別 SoC の上流ループバックケースと同様に、検証用 REPL の FreeRTOS で 8/16 ビット転送、RX/TX 割り込み、DMA を検査。未変更 kit では未接続の応答を確認 |
| CAN | Discovery に専用の上流 ELF ケースは見当たりませんでした。別 SoC の上流 CAN ループバックケースに対応する範囲として、FreeRTOS で bxCAN のフィルター、CAN1 FIFO0、CAN2 FIFO1、送受信・状態 IRQ とペイロードを検査 |
| フラッシュ | `stm32f4-erase.robot` の 3 ケースを Rust で移植。消去モード未指定で値保持、一括消去で両端が `0xffffffff`、セクター 11 消去で先頭の `0xdeadbeef` が残ることを照合。FreeRTOS ELF でも ACR レイテンシ、ロック、オプションバイトを検査。Discovery の標準・カスタムレイテンシ CubeMX ELF は未変更 CPU REPL でそれぞれ 3 行以上の `Hello World!` を照合 |
| 再起動 | 上流 Cortex-M の BASEPRI リセットと STM32L072 の IWDG ケースに相当する検査を F407 FreeRTOS ELF で実施。IWDG のリフレッシュ、期限切れ、SRAM 保持、BASEPRI の `0x50 → 0`、周辺機器の初期状態、再起動後の UART・キュー出力を照合 |
| Cortex-M | `arm-cortex-m-interrupts.robot` の WFE/SEVONPEND、SysTick の RVR/CVR 書き込み順 2 件は `cargo test` で照合。WFE は IRQ 5 の保留後の SRAM 書き込み、SysTick は 100 Hz の 10・1・10・9 を検査 |
| 起動回帰 | 未変更の Contiki RESC で公開 Contiki と Zephyr Hello World を実行。FreeRTOS は未変更 kit の ARM_CM3・ARM_CM4F と検証用 REPL の 3 構成で実行 |

[Discovery Robot ケース](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/STM32F4_Discovery.robot)
[フラッシュ消去](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/stm32f4-erase.robot)
[Cortex-M 割り込みとリセット](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/unit-tests/arm-cortex-m-interrupts.robot)
[STM32L072 IWDG](https://github.com/renode/renode/blob/c38ac3babcd38a7144b9498e3ca4c2226b76aa37/tests/platforms/STM32L072.robot)

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/stm32f407-upstream-smoke.sh target/close-f407-tests
```

公開 ELF は自動取得と SHA-1 照合の対象です。ログは指定ディレクトリに残ります。
IRQ は `faultmask*.log`、I²C/SPI/CAN は `loopback/uart.log`、再起動は各構成の
`watchdog-uart.log` で確認できます。

RIOT は、例外フレームに Thumb 関数ポインターを格納します。例外復帰では保存された
xPSR の Thumb 状態を検査し、PC の下位ビットを落として命令をフェッチします。
RTC のステータスフラグ消去はバックアップドメイン保護中でも受け付けます。
アラームの検査では開始プロンプトの直後に `s` を注入し、UART の行末の仮想時刻が
2・4・6・8 秒台であることと、最終行の後に 3 秒間 UART 出力がないことを 1 回の実行で照合します。

CubeMX の USB 初期化には、上流 REPL の明示的な USB リセットタグと、SVD 由来の
OTG FS リセット値を使用します。参照した `STM32F40x.svd.gz` の SHA-256 は
`d71b3e11c870ec83346eb994e98a57c5eacdf0082d2949ed495d7a3828bf41ec` です。
[SVD アセット](https://dl.antmicro.com/projects/renode/svd/STM32F40x.svd.gz)

受け入れの対象は上表のゲスト起動と入出力です。Discovery の `Peripheral Export File Created` は下記の上流全件の照合に追加しました。
USB データ転送、汎用 SVD 展開、フラッシュ書き込み、CAN 通常モードでのノード間通信は残課題です。
cargo build --release
PENV_ASSET_DIR=/tmp/penv-h7-assets bash tests/stm32h7-ptp-clock-robot.sh "$(mktemp -d)"
```

`cargo test` 241件が通過しました。STM32H7 と LiteX の双方向フレーム交換を含みます。PTP 時計ケースは部分検証、送信2件は未達です。残件は仮想時刻接続、TX 記述子の時刻書き戻し、Sync/Follow_Up のフレームと時刻差、SMMUv3 バイパス構成です。公開 ELF のハッシュ、UART、終了状態は `~/.local/state/penv-dispatch/evidence/h7-ptp.P7JnxuwT` に保存しました。台帳は対象786件のうち等価合格203件、部分検証19件、未達564件です。

PTP 送信側では、通常 TX 記述子で TTSE を要求し、MAC でタイムスタンプを有効にすると、DMA が秒・サブ秒と TTSS を書き戻すようにしました。MMIO 経由でキャプチャ、未要求、無効、コンテキスト記述子の各条件を確認しました。個別再現は `cargo test ptp_tx_descriptor` です。

同じ公開 ELF と上記手順で再実行すると、UART に Sync 送信ログが出ました。時計は 7.48 仮想秒後も `25.000000000` でした。TX の上流2件は、共有仮想時刻の接続、実フレームの Sync/Follow_Up 順序とタイムスタンプの照合、SMMUv3 ケースの実行が残っています。`cargo test` 243件と公開 IS25WP ELF が通過しました。

結果と終了状態は `/home/ec2-user/.local/state/penv-dispatch/evidence/h7-ptp-tx.jduubeFl` に保存しました。

### H753ZI DHCP と長文 UART

```sh
export PENV_ASSET_DIR=/tmp/penv-h7-assets
bash tests/stm32h7-dhcp-robot.sh "$(mktemp -d)"
bash tests/stm32h7-nucleo-uart-line.sh "$(mktemp -d)"
```

固定 Zephyr ELF 2台を未変更 Nucleo REPL と仮想スイッチで実行しました。上流の既定動作と同じく各 UART コマンドのエコーを待つと、サーバーが起動しました。クライアントは15仮想秒のリース待ちでタイムアウトしました。両ノードの MAC は `00:80:e1:ab:10:c1` でした。診断で両スイッチポートを promiscuous にすると `122.101.101.2` を受け取りましたが、クライアントからの ping 応答待ちで停止しました。移植スクリプトは固定プール先頭アドレスを照合します。上流の正規表現による動的なアドレス取得と双方向 ping は残件です。

同じ ELF の長文 UART ケースは、同じ入力、50 ms 後のプロンプト、リングバッファ警告の失敗条件を照合して通過しました。`cargo test` 243件も通過しました。台帳は対象786件のうち等価合格204件、部分検証19件、未達563件です。

ケース別ログと終了状態は `/home/ec2-user/.local/state/penv-dispatch/evidence/h7-dhcp.tHotLqIl` に保存しました。

### 上流全件の照合

Discovery 全 13 ケースと共有候補の個別台帳、確認済みの観測条件、再現手順と未達は
[STM32F407 Discovery 上流ケース台帳](docs/stm32f407-full-upstream.md) に記録しています。
Peripheral Export はこの照合の対象に含め、未変更の CPU REPL からファイルを書き出して検査します。

Cortex-M4F の精密 BusFault と NOCP の ELF 検査は同じ台帳の「Cortex-M4F の精密 BusFault と NOCP」を参照してください。
