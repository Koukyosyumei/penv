# RV64 上流ケース適用範囲監査

固定ソースと対象ボードは [上流ケース台帳](rv64-full-upstream.md) に記載しています。以下は `d735831` 時点の [行単位の台帳](rv64-upstream.tsv) の集計です。後続のベクトル CSR 部分移植 2 件は [ACT とベクトル命令の初期対応](rv64-vector-bootstrap.md) に記録しています。

| 判定 | 行数 |
| --- | ---: |
| 確認済みの対象行（暫定分母） | 1,055 |
| 上流等価の合格 | 31 |
| 部分移植 | 32 |
| 上流等価の合格が未達（部分移植を除く） | 992 |
| 別構成を要求する対象外 | 701 |
| 適用または展開を確認中 | 2,418 |

最終分母は未確定です。確認中の内訳は Renode の 1,886 行、Infrastructure NUnit の 532 行 です。明示的なデータソースの未展開行は今回解消しました。共有リソースやデータ参照、条件で生成する引数、NUnit のディスカバリーを残りの行で検証する必要があります。

## 根拠

- Robot ケースから呼ばれるローカルキーワードと静的に解決できる Resource、固定スカラー引数、設定の Setup、REPL/RESC の参照先を調べます。状態を再利用するケースの適用判定には、同じスイート内の `Provides` と `Requires` も使います。固定構成を特定できない共有ケースは `review` に残します。
- StarLight 7 ケースと Fire 1 ケースは部分移植として保持しています。Fire が取り込む `platforms/cpus/polarfire-soc.repl` の RV64 ハート、MMC、ウォッチドッグを使う PolarFire SoC 7 ケースと Icicle Kit 6 ケースは対象です。Icicle の HSS、FIT、eMMC は各ケースの固定フィクスチャです。
- CLIC の署名試験 144 ケースは、各ケースの ELF と署名入力を台帳に保持しています。各ケースがロードする `CLIC-test-setup.resc` は `CLIC-test-platform.repl` の RV32IMC ハート、未批准の特権仕様、CLIC を固定します。現対象の RV64 PLIC/CLINT 構成からは対象外です。RV32 ハートがあるだけで共有の UART、追跡、モニター等を除外することは避けています。
- Fire の RESC が使う `SdCardFromFile` のファイル処理 3 ケースと、共有 SDCard の eMMC モード 5 ケースを保持しています。FdtSharp の 5 ケースは Zynq の DTB をバイナリ形式の入力に使う fixture で、ボードを起動せず構文とデータを検査するため対象に含めています。
- NUnit の `AccessConditionParserTestCases` の DNF 変換 9 引数は、共有バス登録の条件式に対応する対象です。`TypeExtensionsCacheTests` の 9 引数は .NET のリフレクションと内部統計を要求するため対象外です。`[TestCaseSource]` と `[Test]` が同じメソッドに付く場合はソースの各引数を数えます。
- PeakRDL の `systemrdl.json` 4 件と、メモリー・レジスター用の構造体リテラル 5 件をそれぞれ使用メソッドへ展開しました。生成 C# の型を検査する静的メタデータ検証は対象外です。動的なレジスター挙動とワードアクセスは対象に含めています。
- Packet.Net は IEEE 802.11 の具体的なフレーム・ラジオフィールドと、Linux cooked capture のリンクタイプ 113 を要求する行を区別しました。Ethernet キャプチャのヘッダーからリンクタイプ 1 を確認し、Ethernet 上のプロトコル、チェックサムとバイト順、複合キャプチャのテストを保持しました。Infrastructure の周辺機器テストは NUnit が生成する機器と、3 構成で使用する REPL の機器を照合しました。
- 固定 ISA 372 本と固定 ACT 242 本は上流セレクターで展開しました。Renode の参照側結果と Penv の診断は [ACT 構成・署名の照合](rv64-act-equivalence.md) を参照してください。Penv の固定 ACT には V の実装が残り、診断の合格を上流等価の合格には加算していません。

## 再現

[固定ソースと ELF の準備・台帳生成コマンド](rv64-full-upstream.md#再現)を使用します。台帳生成を繰り返し、出力を `cmp` で比較します。監査時は Robot Framework のパーサーで固定 Renode のケース名集合を照合し、2,203 組のファイル・ケース名がすべて台帳に存在することを確認しました。この照合は名前の網羅性を検証します。

以下のログは `target/scope-audit-rv64/` に保存しました。`$virt_work`、`$fire_assets`、`$starlight_assets` は README の公開ファームウェア素材ディレクトリです。

```sh
out=target/scope-audit-rv64
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt"
python3 scripts/verify-fire.py "$fire_assets" --max-steps 9000000000 \
  --timeout 1800 --logs "$out/fire"
python3 scripts/verify-starlight.py "$starlight_assets" --upstream-cases \
  --ports 26917 26918 --switch-port 26919 --logs "$out/starlight"
python3 scripts/probe-rv64-isa.py --act --smepmp "$act/work" --ram-size 0x400000 \
  --elf spike-rv64-max/elfs/priv/Sm/Sm-00.elf --logs "$out/act-sm"
```

`cargo test` は 107 件合格。RV64 virt は OpenSBI から Linux の UART ログインへ到達し、`id -u` が `0`、`uname -m` が `riscv64` を返しました。公開 ACT の `Sm-00.elf` は 4 MiB の Smepmp 診断構成で署名と終了を確認しました。

StarLight の通常構成では 2 台で root ログインし、56 バイトと 3,200 バイトの ping が各 10/10 応答しました。両ゲストで Ethernet IRQ の増加を確認しました。ログは `starlight.log` と `starlight/` にあります。

Fire の今回の再試行では公開 SD から HSS、OpenSBI、U-Boot、Linux 6.6.35、SD のパーティション列挙まで確認しました。試行時の既定値である 60 億命令に達し、ログイン前に終了しました。ログは `fire.log`、`fire/fire.uart.log`、`fire/fire.stderr.log`、終了状態は `fire.status` にあります。上記の再現コマンドは、ログインと SD のセクターハッシュ照合まで成功した既存検証と同じ上限を指定しています。[既存の公開 Fire イメージの検証結果](../README.md#上流-renode-の-rv64-ケース対応)

## RV64 ISA 文字列の互換性

固定 Renode の `riscv-isa-string.robot:75` は、大小文字が混在する入力で RV32 と RV64 の CPU を構築します。Penv の RV64 ISA 解析は大小文字を区別せず処理するように変更しました。固定上流ケースの実行と、Penv で同じ RV64 文字列を使った公開 `rv64ui-p-add` ELF の終了値 0 を確認しました。台帳の状態は上流ケース全体に対する部分移植です。ログは `upstream-isa-string.log`、`mixed-isa-elf.log`、`cargo-test-final.log` です。

```sh
renode-test "$renode_work/tests/unit-tests/riscv-isa-string.robot" \
  --fixture 'Should Handle Both 32 And 64 Bit Versions' --results-dir "$out/upstream-isa-string" -P 0
sed 's/"rv64gc"/"RV64IMAFDC_Zicsr_Zifencei"/' platforms/rv64_virt.repl > "$out/mixed-isa.repl"
elf="$work/isa/rv64ui-p-add"
tohost=$(llvm-nm "$elf" | awk '$3=="tohost" {print "0x" $1}')
target/debug/penv run --platform "$out/mixed-isa.repl" --max-steps 100000 --tohost "$tohost" "$elf"
```
