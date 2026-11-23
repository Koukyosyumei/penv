# RV32 上流互換検証

対象は LiteX/VexRiscv、Minerva、Arty、Nexys、RV32 virt と、それらに適用できる ISA・特権・割り込みのケースです。固定したソースは次のとおりです。

| ソース | リビジョン |
| --- | --- |
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` |
| Renode Infrastructure | `09ac7c563095c5410258ecc1b1a6b841a013db85` |
| Infrastructure 内 tlib | `c04522a9c886ba743c77a86b0c879103a2ef46d0` |
| Renode UI | `5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f` |
| UI 内 aesir | `9b5b336decdd5d0f30f98342347f83751dd9ec21` |
| ACT | `1dfa48c2adecaa16fae9fca14d34c591ea99321c` |
| ACT 内 UDB | `285e23a1a3f1df8114df2dcf9463f05d8b507026` |

## 台帳の使い方

[riscv-upstream-cases.tsv](riscv-upstream-cases.tsv) は固定スナップショットから生成した棚卸しです。Robot のケース名と明示タプル、NUnit のメソッド名と静的な `TestCase`・`Values`・整数 `Range` を出典行へ対応づけています。ISA/ACT のファイル列挙には固定 Renode の Python モジュールを使います。tlib のテスト名に該当するファイルは Xtensa の実装および C のハッシュテーブル検査です。aesir に Robot/NUnit のソースはありません。関連性の判定が済んでいない共有ケースも台帳に残しています。

- `target` は対象、`review-variants` はモデルを明示する共有スイート、`review-shared` は汎用スイートの適用候補です。後者二つは個別の環境・パラメータを要確認です。
- `outside` は RV64 や対象外 CPU を明示する行に理由を添えています。
- `ported` は入力・期待値・失敗条件を対応づけた移植です。`partial` は一部の照合、`blocked-cpu` は上流構成の CPU 作成失敗、`pending` は未判定、`missing-input` は実ファームウェアが揃っていない動的ケースです。
- NUnit の `TestCaseSource` と未展開の引数属性、Robot のループ・実行時変数から生じるパラメータは追加展開が必要です。機種固有スイートの既存回帰も上流の順序・時間・否定条件をケースごとに照合する必要があります。

## 再現手順

`../renode` は読み取り専用の参照です。サブモジュールと ACT は別ディレクトリに取得します。

```bash
git init "$work/infrastructure"
git -C "$work/infrastructure" fetch --depth=1 https://github.com/renode/renode-infrastructure.git 09ac7c563095c5410258ecc1b1a6b841a013db85
git -C "$work/infrastructure" checkout --detach FETCH_HEAD
git init "$work/ui"
git -C "$work/ui" fetch --depth=1 https://github.com/antmicro/renode-ui.git 5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f
git -C "$work/ui" checkout --detach FETCH_HEAD
git init "$work/act"
git -C "$work/act" fetch --depth=1 https://github.com/riscv-non-isa/riscv-arch-test.git 1dfa48c2adecaa16fae9fca14d34c591ea99321c
git -C "$work/act" checkout --detach FETCH_HEAD
```

固定 ACT の CI が使用する RISC-V GCC 15.1.0（2025.07.16 nightly）と Sail 0.9、`uv`、UDB 用コンテナを用意し、次を実行します。

```bash
export PATH="$work/tools/bin:$PATH"
DOCKER=1 make -C "$work/act" -B testgen EXTENSIONS=
DOCKER=1 make -C "$work/act" generate-makefiles-dut EXTENSIONS= \
  CONFIG_FILES=config/spike/spike-rv32-max/test_config.yaml
# GCC と Sail の出力はそのまま使い、生成後の付随する逆アセンブリだけ省略した。
sed -i "s|$work/tools/bin/riscv64-unknown-elf-objdump|/usr/bin/true|g" \
  "$work/act/work/common/Makefile-rv32i.mk" "$work/act/work/spike-rv32-max/Makefile"
make -C "$work/act/work" -j8 compile
```

README に記載した SHA-256 照合済みの公開 ISA アーカイブを展開します。Robot Framework をインストールした Python で台帳を生成します。

```bash
mkdir -p "$work/isa" "$work/act/work"
tar xzf "$work/riscv-tests.tar.gz" -C "$work/isa" --wildcards './rv32*'
python3 scripts/inventory-riscv-upstream.py ../renode "$work/infrastructure" "$work/ui" \
  "$work/isa" "$work/act/work" > "$work/riscv-upstream-cases.tsv"
cargo test --locked
cargo build --locked --release
sed 's/"rv32imac"/"rv32imac_zicsr_zifencei_zicond_zbs"/' \
  ../renode/platforms/cpus/riscv_virt.repl > "$work/zbs.repl"
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$work/zbs.repl" --artifacts "$work/zbs-results" \
  "$work"/isa/rv32uzbs-[pv]-*
bash scripts/validate-riscv-upstream.sh "$PWD/target/release/penv" ../renode "$work/litex"
```

上流の CPU とメモリ構成での試行は次のとおりです。ACT の UART 定義は Robot の設定を使います。

```bash
cat > "$work/act-upstream.repl" <<'REPL'
cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp"
uart: UART.NS16550 @ sysbus 0x10000000
mem: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x100000
REPL
sed '/^uart:/d' "$work/act-upstream.repl" > "$work/isa-upstream.repl"
mapfile -t act < <(awk -F '\t' -v base="$work/act/work" \
  '$4 == "Should Run Arch Tests 32" && $5 != "" { print base "/" $5 }' "$work/riscv-upstream-cases.tsv")
mapfile -t isa < <(awk -F '\t' -v base="$work/isa" \
  '$4 == "Should Run Tests 32" && $5 != "" { print base "/" $5 }' "$work/riscv-upstream-cases.tsv")
test "${#act[@]}" -gt 0 && test "${#isa[@]}" -gt 0
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$work/act-upstream.repl" --artifacts "$work/act-upstream" "${act[@]}"
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$work/isa-upstream.repl" --artifacts "$work/isa-upstream" "${isa[@]}"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --platform "$work/zbs.repl" --artifacts "$work/act-virt-diagnostic" "${act[@]}"
```

## 結果と残件

公開 ISA アーカイブを固定 Robot の discovery に通すと RV32 は 290 ファイルです。virt 設定での診断は修正前 146 件、修正後 162 件が合格し、非整列アクセス設定を追加した別実行で `ma_data` 2 件が合格しました。Zbs の 16 件は修正前に失敗し、修正後は `tohost=0` を報告しました。Zbs を指定した CPU で BCLR、BEXT、BINV、BSET のレジスター・即値形式を有効にしました。固定 Robot が指定する最大構成 `rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp` は CPU 作成時に拒否されます。この診断の合格は台帳上の Robot 合格に算入しません。

LiteX I²S の `Should Echo Audio` は同じ公開入力・ELF と仮想時間 3.2 秒を使い、送信機を Dispose した後に `cmp` で長さと全バイトを照合します。他の LiteX 回帰については上流ケースごとの等価性確認が残ります。Minerva では公開 BIOS の UART ロードを維持しています。

固定 ACT では UDB コンテナ `ghcr.io/riscv/udb:0.19`（digest `sha256:693d8070498ac64263cb7ab0684024fc89985e13fbd24712142fcfd02eb7474b`）で構成検証し、Sail 参照署名付きの `spike-rv32-max` を生成しました。固定 discovery は 510 件を列挙しました。この中には `elfs/` の 487 件に加え、既定の `work/` 全体を指定したときに拾う `build/` の署名用中間 ELF が 23 件あります。上流 CPU 設定では ISA 290 件、ACT 510 件すべてが CPU 作成時に失敗しました。virt 設定と ACT パケット方式での別診断は 150 件が合格、360 件が失敗しました。

全件合格の判定には、上流の最大 ISA 構成と未対応命令・CSR、共有ケースの適用判定と動的パラメータの展開、機種固有ケースの観測条件の照合・移植が残っています。

## 最大 ISA 構成の切り分け

`full-rv32-isa-config` では同じ上流 REPL を使い、`cpuType` だけを差し替えて、公開 `rv32ui-p-add` ELF の開始までを確認しました。

| 設定または機能 | 観測・対応 |
| --- | --- |
| `rv32g`、`rv32imaf`、`rv32imafd` | G は IMAFD と Zicsr/Zifencei を要求する。F・D の命令と浮動小数点状態が未対応 |
| `rv32imacv` | V の命令とベクター状態が未対応 |
| `rv32imac_zfh` | 半精度浮動小数点が未対応 |
| `rv32imac_zacas` | `amocas` が未対応 |
| `rv32imac_smepmp` | 標準 PMP に加えて必要な `mseccfg` と Smepmp のアクセス規則が未対応 |
| `rv32imac_zba`、`rv32imac_zbb`、`rv32imac_zbc` | 実装して各拡張を有効化できることを確認。複数のビット操作拡張を同時指定できる |
| `rv32imac_zbs` | 既存実装を同じ構成に含め、回帰を確認 |
| 上流 ACT の `UART.NS16550` | 最大 ISA を切り分けて縮小した設定で PLIC 必須エラーを再現。IRQ 配線がない UART はポーリングで使用できるようにした |

同じ ELF とメモリ配置を使い、上流 ACT 510 件、上流 ISA 290 件を再試行しました。等価条件での合格はそれぞれ **0/510 と 0/290** です。いずれも CPU 作成時に F・D・V・Zfh・Zacas・Smepmp が未対応と報告されたため、台帳は `blocked-cpu` を維持します。ACT の署名用中間 ELF 23 件も上流 discovery の結果に従って実行対象に残しました。

縮小 ISA での診断では、Zba/Zbb/Zbc/Zbs の公開 ELF 64 件と、固定 ACT の自己検証 ELF 32 件がすべて `tohost=0` を返しました。これらの 96 行には診断結果を追記しています。ACT 510 件を縮小 ISA と上流のメモリ・UART 配置で試すと 145 件が通過しました。別途、前回と同じ virt REPL に実装した拡張を追加した診断は 150 件から 183 件に増え、以前の通過ケースはすべて維持しました。

上段の再現手順で作った `act-upstream.repl` と配列 `act`・`isa` を使用します。

```bash
for variant in rv32g rv32imaf rv32imafd rv32imacv rv32imac_zfh rv32imac_zacas rv32imac_smepmp \
               rv32imac_zba rv32imac_zbb rv32imac_zbc rv32imac_zbs; do
  sed "s/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/$variant/" "$work/act-upstream.repl" \
    > "$work/$variant.repl"
  target/release/penv run --platform "$work/$variant.repl" --max-steps 1 "$work/isa/rv32ui-p-add" \
    || printf '%s exit=%s\n' "$variant" "$?"
done

sed 's/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/rv32imac_zicsr_zifencei_zicond_zba_zbb_zbc_zbs/' \
  "$work/act-upstream.repl" > "$work/bitmanip.repl"
mapfile -t bitmanip_isa < <(printf '%s\n' "${isa[@]}" | awk -F / '$NF ~ /^rv32uzb[abcs]-[pv]-/')
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$work/bitmanip.repl" --artifacts "$work/isa-bitmanip" "${bitmanip_isa[@]}"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --platform "$work/bitmanip.repl" --artifacts "$work/act-bitmanip" \
  "$work"/act/work/spike-rv32-max/elfs/rv32i/Zb[abcs]/*.elf
```

ACT 510 件の追加診断では、`--act` を指定して `bitmanip.repl` と、以下の virt REPL のそれぞれで上段の `act` 配列を実行します。上流の最大 ISA の試行は上段の `act-upstream.repl` を使用します。

```bash
sed 's/"rv32imac"/"rv32imac_zicsr_zifencei_zicond_zba_zbb_zbc_zbs"/' \
  ../renode/platforms/cpus/riscv_virt.repl > "$work/virt-bitmanip.repl"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --platform "$work/virt-bitmanip.repl" --artifacts "$work/act-virt-bitmanip" "${act[@]}"
```

## RV32 Zacas と CPU 作成の再検証

`full-rv32-cpu-config` は前回の最大構成の先頭ログ `0001-ExceptionsS.sig.elf.log` から着手しました。CPU 作成時の診断に出ていた Zacas を `rv32imac_zacas` で単独再現し、RV32 の `AMOCAS.W` と `AMOCAS.D` を実装しました。A 拡張を必要とし、D の偶数レジスター対、x0 対、自然境界、読み書き権限を反映します。64 ビット CAS はシミュレータの命令実行中に一括して処理します。[RISC-V Zacas 仕様](https://docs.riscv.org/reference/isa/v20260120/unpriv/zacas.html)

固定 ACT の `Zacas-amocas.w-00.elf` は変更前に CPU 作成で停止し、変更後は `tohost=0` で通過しました。D の成功・比較不一致、x0 対、奇数レジスター、非整列、拡張無効時の例外は Rust テストで確認しました。上流 `riscv-amocas.robot` の該当 D ケースは台帳で部分検証として記録しています。

```bash
cargo test --locked
cargo build --locked --release
sed 's/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/rv32imac_zacas/' \
  "$work/act-upstream.repl" > "$work/zacas.repl"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --platform "$work/zacas.repl" --artifacts "$work/zacas-results" \
  "$work"/act/work/spike-rv32-max/elfs/rv32i/Zacas/*.elf
```

上段の最大構成の手順でも ACT 510 件、ISA 290 件を再実行しました。上流構成の通過は引き続き各 0 件です。両方とも F・D・V・Zfh・Smepmp が未対応で CPU 作成に失敗し、台帳では `blocked-cpu` を維持しています。固定 ACT の W の結果は縮小構成の診断です。

## RV32 F/D の初期化と演算

`full-rv32-fd-bootstrap` では RV64 の `rv64_float.rs`（`d81737d607dd11ddc593ac22248588c0dc114d12`）から整数仮数による丸め・演算・例外フラグを移植しました。RV64 側の命令デコーダ（`0712104489a2342fc63c29eda1a0b41710a7518c`）は命令の対応関係を参照し、RV32 の整数レジスター幅、Sv32 メモリアクセス、圧縮ロード・ストア、`mstatus/sstatus.FS` と `fcsr/frm/fflags` は RV32 側で接続しました。F は単精度を有効にし、D は F と倍精度を有効にします。半精度や RV64 の整数変換・倍精度レジスター転送エンコーディングは無効です。

| 構成 | 結果 |
| --- | --- |
| 上流最大 ISA、RAM 1 MiB | ACT 0/510、ISA 0/290。CPU 作成時に V・Zfh・Smepmp が未対応 |
| `rv32imafc`、1 MiB | ACT F 26/38、Zcf 4/4 |
| `rv32gc_zba_zbb_zbc_zbs_zacas`、1 MiB | ACT D 52/64、Zcd 4/4 |
| 同じ F/D 縮小 ISA、2 MiB | ACT F 38/38、D 64/64。公開 ISA F/D 42/42 |
| 同じ D 縮小 ISA、2 MiB で ACT 全件を診断 | 254/510 |

F のみの構成で D の `D-fadd.d-00.elf` を実行すると初期化時の `FLD` が違法命令になります。1 MiB で失敗した F と D 各 12 件は、ELF のロードセグメント終端が `0x80100000` を超えます。2 MiB はそれらの演算・フラグを確認するための診断です。最大 ISA 構成の合格判定は `blocked-cpu` のまま、F/D の構成別結果を台帳に記録しています。

```bash
cargo test --locked
cargo build --locked --release
sed 's/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/rv32imafc/' \
  "$work/act-upstream.repl" > "$work/f.repl"
sed 's/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/rv32gc_zba_zbb_zbc_zbs_zacas/' \
  "$work/act-upstream.repl" > "$work/fd.repl"
for profile in f fd; do
  sed 's/size: 0x100000/size: 0x200000/' "$work/$profile.repl" > "$work/$profile-2m.repl"
done
for group in F D Zcf Zcd; do
  profile=fd
  case $group in F|Zcf) profile=f ;; esac
  python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
    --platform "$work/$profile.repl" --artifacts "$work/$group" \
    "$work"/act/work/spike-rv32-max/elfs/rv32i/"$group"/*.elf || test "$?" -eq 1
  case $group in F|D)
    python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
      --max-steps 10000000 --platform "$work/$profile-2m.repl" --artifacts "$work/$group-2m" \
      "$work"/act/work/spike-rv32-max/elfs/rv32i/"$group"/*.elf
    ;;
  esac
done
mapfile -t fp_isa < <(find "$work/isa" -maxdepth 1 -type f -name 'rv32u[fd]-*' ! -name '*.dump' | sort)
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$work/fd-2m.repl" --artifacts "$work/isa-fd" "${fp_isa[@]}"
```

V・Zfh・Smepmp の実装、上流構成での再検証、上流の RAM 容量と生成 ELF の整合確認が残っています。

## RV32 Zfh

`full-rv32-zfh` では既存 F/D の演算・丸め・フラグ処理に半精度形式を追加しました。`_zfh` には F が必要です。半精度の NaN ボックス化、FLH/FSH、FMV.H.X と FMV.X.H、半精度演算、比較・分類、整数および F/D との変換を同じ浮動小数点状態と CSR に接続しました。Zfh 無効時の FLH は違法命令になり、F を欠く `rv32imac_zfh` は作成時に拒否されます。

| 固定 ACT ケース | 縮小 ISA | RAM 1 MiB | RAM 2 MiB |
| --- | --- | ---: | ---: |
| Zfh | `rv32imafc_zfh` | 41/53 | 53/53 |
| ZfhD | `rv32gc_zfh` | 30/30 | 30/30 |
| Zfhmin | `rv32imafc_zfh` | 6/6 | 6/6 |
| ZfhminD | `rv32gc_zfh` | 6/6 | 6/6 |

Zfh の残り 12 件はロードセグメントが 1 MiB の RAM 範囲を超えます。2 MiB で全件が自己検証を通過しました。Zfhmin 系の ELF も Zfh を有効にした構成で検証しました。公開 ISA の Zfh 22 件は `rv32gc_zfh` と 2 MiB で全件が `tohost=0` を返し、既存 F/D ACT は 38/38 と 64/64 を維持しました。

```bash
cargo test --locked
cargo build --locked --release
for spec in fh:rv32imafc_zfh fdh:rv32gc_zfh; do
  profile=${spec%:*}
  sed "s/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/${spec#*:}/" \
    "$work/act-upstream.repl" > "$work/$profile.repl"
  sed 's/size: 0x100000/size: 0x200000/' "$work/$profile.repl" > "$work/$profile-2m.repl"
done
for group in Zfh ZfhD Zfhmin ZfhminD; do
  profile=fh
  [[ $group != *D ]] || profile=fdh
  for size in '' -2m; do
    python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
      --max-steps 10000000 --platform "$work/$profile$size.repl" --artifacts "$work/$group$size" \
      "$work"/act/work/spike-rv32-max/elfs/rv32i/"$group"/*.elf || test "$?" -eq 1
  done
done
mapfile -t zfh_isa < <(find "$work/isa" -maxdepth 1 -type f -name 'rv32uzfh-*' ! -name '*.dump' | sort)
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$work/fdh-2m.repl" --artifacts "$work/isa-zfh" "${zfh_isa[@]}"
```

上流の最大構成では ACT 0/510、ISA 0/290 です。V と Smepmp が引き続き CPU 作成を阻んでいます。縮小構成の結果と RAM 差は台帳に残し、最大構成の等価合格数に含めていません。

## 適用範囲と分母の監査

`scope-audit-riscv` では、同じ上流ケースの同じパラメータを 1 行として数えます。大文字・小文字が異なる ISA 文字列は、構文解析の入力自体が異なるため別ケースです。反復が一つの主張の手順を構成する場合は同じケースに属し、CPU・アドレス・命令幅ごとに独立した期待値を検査する LR/SC は展開します。

- `riscv-lr-sc-strong-atomicity.robot` は実 REPL の `cpu32_0/cpu32_1` と `cpu_0/cpu_1`、RAM と MMIO、介入する書き込みから 26 組を展開しました。RV32 の 12 組を対象に残し、RV64 の 14 組に別構成の根拠を書きました。
- CLINT、割り込みモード、外部 PMP、ISA 構文解析の RV32 ケースは固定の CPU や引数を照合し、該当する未実装ケースを対象に残しました。RV32 の Andes 拡張テストも汎用 `CPU.RiscV32` へ渡す条件として対象に残しました。
- CLIC ACT の各ケースは個別の ELF・署名と、`CLIC-test-setup.resc` が読み込む `CLIC-test-platform.repl` を照合しました。固定ケースは標準 virt の PLIC/CLINT と異なる CLIC・未批准特権構成を必要とします。Andes PLIC のトリガー型レジスター、Ibex、Microwatt、CV32E40P、固定 RV64 構成も、該当するフィクスチャまたは実際のキーワード引数を根拠にしています。
- NUnit の固定 `TestCaseSource` 2 箇所は各 9 組に展開しました。Robot の ISA 構築テストでは同一入力 `rv32gc` の繰り返し 1 件を除きました。
- LiteX の既存検証は、同じ公開バイナリで一部を確認したケースを部分移植として扱います。例えば Nexys Video は画像一致を確認済みで、Robot のモニター停止・タイムアウト条件の等価性は確認が残ります。

| 判定 | 台帳行数 |
| --- | ---: |
| 確認済みの対象実行ケース | 1,076 |
| うち上流等価の合格 | 1 |
| うち部分移植 | 27 |
| うち未達 | 1,048（最大 ISA で CPU 作成失敗 800、未検証または未移植 248） |
| 別構成を必要とする対象外 | 329 |
| 適用判定または展開が未確定 | 2,778 |

1,076 件は現時点で確定した対象の下限です。共有 Robot の呼び出し先・動的変数・モニター条件と、共有 NUnit の対象モデルとの対応が未確認です。未展開のケースから追加の構成が出るため、台帳の 4,183 行を最終分母にはできません。

台帳は上段の固定ソースとファームウェアから同じ生成コマンドで再現できます。重複は `component/file/case/parameter` の組で照合し、実ファームウェア回帰には `scripts/validate-riscv-upstream.sh` を使いました。

## RV32 Smepmp

RV64 の `c6889904cd78b8e29e343a76c403540f4b54ac09` から MML の権限表、MML/MMWP の固定化、RLB とロック済み TOR の規則を移植しました。RV32 では `mseccfg` に加え `mseccfgh` を実装し、Smepmp の上位ビットは 0 として読み戻します。PMP エントリーを持つ CPU で `_smepmp` を指定します。[RISC-V Smepmp 仕様](https://docs.riscv.org/reference/isa/v20260120/priv/smepmp.html)

- RV64 の物理アクセス検査を RV32 ELF に移植しました。許可された共有データの読み書きと、未一致領域の拒否を確認し、ロード・ストア・フェッチの `mcause=5/7/1`、`mtval`、`mepc` を照合しました。
- 固定 Renode `pmp.robot` の RV32 ケースは、固定アドレスと NAPOT の設定値・テストデータを使い、U モードからの許可、全エントリー OFF 時の例外、PMP がない場合の許可を Rust で確認しました。縮小 ISA を使った部分移植として台帳に反映しました。
- Smepmp 付き縮小構成で公開 M/S 特権 ISA ELF は 22/22 件、固定 ACT は 317/510 件が通過しました。固定 `Sm-00.elf` は RAM 1 MiB と 4 MiB の両方で test 34 の `mstatus` 読み戻しが期待値 `0x00000000` に対して `0x00001800` となり、後続の `mseccfg` ケースには到達しませんでした。
- 上流最大構成は ACT 0/510、ISA 0/290 件です。V が CPU 作成を阻むため、縮小構成の合格とは区別しています。

```bash
cargo test --locked
cargo build --locked --release
sed 's/rv32gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp/rv32gc_zba_zbb_zbc_zbs_zfh_zacas_smepmp/' \
  "$work/act-upstream.repl" > "$work/smepmp.repl"
sed 's/size: 0x100000/size: 0x400000/' "$work/smepmp.repl" > "$work/smepmp-4m.repl"
clang --target=riscv32-unknown-elf -march=rv32i -mabi=ilp32 \
  -nostdlib -fuse-ld=lld -Wl,-T,scripts/firmware/rv32-virt.ld \
  scripts/firmware/rv32-smepmp.S -o "$work/rv32-smepmp.elf"
status=0
target/release/penv run --platform "$work/smepmp.repl" --max-steps 1000 \
  "$work/rv32-smepmp.elf" > "$work/smepmp.uart" || status=$?
test "$status" -eq 2
grep -Fx 'rv32-smepmp: allowed RW, denied R/W/X with causes 5/7/1' "$work/smepmp.uart"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --platform "$work/smepmp.repl" --artifacts "$work/act-smepmp" "${act[@]}"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --max-steps 10000000 --platform "$work/smepmp-4m.repl" --artifacts "$work/act-sm-4m" \
  "$work"/act/work/spike-rv32-max/elfs/priv/Sm/*.elf
```

台帳で適用を確認済みの下限は 1,079 件（上流等価合格 1、部分移植 31、未達 1,047）です。対象外 331 件、適用または展開が未確定の行が 2,773 件残っています。

## 固定 Sm-00 の mstatus 観測値

`Sm-00.S` の test 34 は、開始時に読み取った `mstatus` から SD・FS・XS・VS を消し、MPP を保持した値を書き戻します。固定 Sail の `Sm-00.results` 35 行目は `0x00000000` です。固定 Renode の tlib `arch/riscv/cpu.h` の `set_default_mstatus()` は MPP を M に設定します。penv の既定値でも test 34 は `0x00001800` を読み戻しました。

標準 RV32 REPL の `init:` で `MSTATUS VALUE` を受け付け、ISA と特権レベルを設定した後に通常の CSR 書き込み規則を適用します。この初期化を省略した場合、従来どおり MPP は M です。Sail と同じ初期値を明示した診断構成では test 34 を通過し、test 35 で `VS=01` の期待値 `0x00000200` に対し `0x00000000` を読み戻しました。最大構成が必要とする V は未実装のため、この先の Smepmp 署名と上流等価の合格は未確認です。

```bash
# smepmp.repl は上段の Smepmp 節で生成した縮小 ISA、RAM 1 MiB の REPL
sed '/cpuType:/a\    init:\n        MSTATUS 0' "$work/smepmp.repl" > "$work/smepmp-sail-init.repl"
for profile in smepmp smepmp-sail-init; do
  python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
    --max-steps 10000000 --platform "$work/$profile.repl" --artifacts "$work/sm-$profile" \
    "$work"/act/work/spike-rv32-max/elfs/priv/Sm/Sm-00.elf || test "$?" -eq 1
done
```

公開特権 ISA は両方の初期化で各 22/22 が通過しました。RV32 Smepmp 検証 ELF も両構成で許可領域の読み書きと例外 `5/7/1` を再確認しました。

## RV32 ベクトル初期化

固定 ACT の `tests/env/utils.h` の `RVTEST_V_ENABLE` は、`mstatus.VS` を Initial にして `vlenb` を読みます。`test_setup.h` は `RVTEST_VECTOR` を指定したテストでこのマクロを呼びます。今回の固定 ELF 群で確認した先頭の要求は、`Sm-00.S` が test 35 以降で行う `mstatus.VS` の読み戻しです。`UV.S` は U モードへ入り M モードへ戻りますが、ベクトル演算の本体は上流ソース上でスタブです。固定 ELF 群の結果からデータ演算を検証したとは判定できません。

標準 RV32 の診断 REPL に `vectorBootstrap: true` を追加しました。固定 Spike ACT プロファイルに合わせた VLEN 1024、ELEN 64 で、`mstatus/sstatus.VS` と SD、`vstart/vxsat/vxrm/vcsr/vl/vtype/vlenb`、`vsetvli/vsetivli/vsetvl` を実装しています。このオプションは `misa.V` を設定しません。通常の CPU 型の V は引き続き作成時に拒否されます。[RISC-V V 仕様](https://docs.riscv.org/reference/isa/unpriv/v-st-ext)

| 検証 | 結果 |
| --- | --- |
| 固定 `Sm-00.elf`、Sail 初期値とベクトル診断、RAM 1 MiB | 署名と `tohost=0` を確認 |
| 固定 `UV.elf`、同じ構成 | `0x02004000` へのアクセスが未マップ |
| 固定 `UV.elf`、CLINT を追加 | `tohost=0`。ベクトル演算の検査なし |
| 上流最大構成、Sm と UV を抜き取り | 0/2、V により CPU 作成時に拒否 |
| 検証 ELF | ACT の初期化列、CSR、3 種の `vset*`、不正 `vtype` を確認。オプションを外すと `vlenb` で違法 CSR |

```bash
cargo test --locked
cargo build --locked --release
# smepmp-sail-init.repl は上段で生成済み
sed '/cpuType:/a\    vectorBootstrap: true' "$work/smepmp-sail-init.repl" > "$work/vector-init.repl"
"$work/tools/bin/riscv64-unknown-elf-gcc" -march=rv32gcv -mabi=ilp32 -nostdlib \
  -Wl,-T,scripts/firmware/rv32-virt.ld scripts/firmware/rv32-vector-bootstrap.S -o "$work/vector.elf"
status=0
target/release/penv run --platform "$work/vector-init.repl" --max-steps 1000 "$work/vector.elf" \
  > "$work/vector.uart" || status=$?
test "$status" -eq 2
grep -Fx 'rv32-vector-bootstrap: VS, vlenb, vsetvl and vector CSRs OK' "$work/vector.uart"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --max-steps 10000000 --platform "$work/vector-init.repl" --artifacts "$work/vector-sm" \
  "$work"/act/work/spike-rv32-max/elfs/priv/Sm/Sm-00.elf
sed '/cpuType:/a\    timeProvider: clint' "$work/vector-init.repl" > "$work/vector-clint.repl"
cat >> "$work/vector-clint.repl" <<'REPL'
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x02000000
    [0, 1] -> cpu@[3, 7]
    frequency: 10000000
REPL
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --max-steps 10000000 --platform "$work/vector-clint.repl" --artifacts "$work/vector-uv" \
  "$work"/act/work/spike-rv32-max/elfs/priv/UV/UV.elf
```

ベクトルレジスター、演算・ロード／ストア、上流のベクトル演算ケースの生成・検証が残ります。二つの診断結果を台帳へ記録し、上流最大構成の合格には算入していません。

## RV32 割り込みモードの CSR 書き込み

固定 `tests/unit-tests/riscv-interrupt-mode.robot` のソフトウェア書き込み 6 件を移植しました。上流 REPL の `CPU.RiscV32`、`rv32imac`、Priv1_10、`timeProvider: empty` と、Auto・Direct・Vectored の設定を使用します。明示モードに従って MTVEC/STVEC の下位ビットを補正し、CSR 書き込み値を変更したときに警告を出します。モード指定を省略した既存構成は従来の動作です。

検証器は固定 Robot の命令語、初期値、5 回の書き込み後の読み戻し値、各回の警告条件を取り出します。各ケースを一つの ELF として実行し、全読み戻しが一致した場合の `tohost=0` と、余分な補正警告がないことを確認します。6 件が通過しました。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-interrupt-mode.py target/release/penv ../renode "$work/interrupt-mode"
```

モニター書き込み 6 件は、実行中の設定・照会とログの順序を検査する等価な実行経路が残っています。今回の ELF 検証はソフトウェア書き込み 6 件に対応します。公開 M/S 特権 ISA 22 件と、virt の CLINT・PLIC・NS16550 実 ELF も通過しました。上流最大 ISA の V と、共有ケースの適用・展開判定は引き続き残件です。

台帳で適用を確認済みの下限は 1,079 件で、上流等価合格 7 件、部分移植 31 件、未達 1,041 件です。

### モニター経由の MTVEC/STVEC

起動 RESC の `cpu MTVEC [VALUE]` と `cpu STVEC [VALUE]` を、単一マシン上で記述順に適用します。値を省くとその時点の値を表示します。補正が生じた場合はモニター経由の警告を出し、明示的な CLI `--mtvec` は起動 RESC の操作後に適用します。

上段の `validate-rv32-interrupt-mode.py` は固定上流からモニター 6 件も読み込みます。各ケースで上流と同じ初期値、5 回の書き込み、読み戻し、警告の有無と順序、Vectored の初期化メッセージを一つの起動スクリプトで照合し、前回移植したゲスト CSR 書き込み 6 件も再実行します。全 12 件が通過しました。実ファームウェアでは `cpu MTVEC` を設定する上流 Murax RESC の UART エコーと、RV32 virt の CLINT・PLIC 割り込みを確認しました。

このグループの 12 件は上流等価として台帳に記録しています。台帳全体は上流等価合格 13 件、部分移植 31 件、未達 1,035 件です。最終分母と、V を含む最大 ISA 構成は引き続き残件です。

### RV32 ISA 構築時のエラー

固定 Robot の `riscv-isa-string.robot` と `riscv-extensions.robot` から、無効な ISA を渡す 5 件を移植しました。I/E の同時指定、RV32E に対する無効な標準拡張、未定義の拡張名を区別します。変更前は 5 件とも汎用的な未対応 CPU 型の診断で、上流が要求する診断を含みませんでした。

`validate-rv32-isa-errors.py` は上流から文字列と期待する診断を抽出し、CPU 作成が終了コード 1 と指定メッセージを返すことを検証します。CLI が構成チェックに必要とする最小 RAM とダミー入力を添えています。判定はファームウェアの実行前に行います。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-isa-errors.py target/release/penv ../renode "$work/isa-errors"
```

正当な ISA 文字列の構築テストには、RV32E、V とベンダー拡張、区切りを省いた表記などの未対応行が残っています。これらは対象のままとし、拒否ケースの合格に含めていません。台帳全体は上流等価合格 18 件、部分移植 31 件、未達 1,030 件です。最終分母は未確定です。

### RV32 の既存機能を指定する ISA 表記

固定 `riscv-isa-string.robot` の構築ケースのうち、既存の命令セットだけを必要とする 29 件を検証しました。変更前に拒否された 8 件を含め、全件が CPU を作成できました。単文字拡張の順序、G の展開、`Zicsr` の直前や `RV32` の直後の区切り、A を M なしで指定する構成を扱います。B は固定 Renode の実装に合わせて Zba/Zbb/Zbc/Zbs を有効にします。

検証器は固定 Robot の元の表記を保持し、同じ文字列が重複する場合は一度だけ実行します。構築後に最小 RAM 上の EBREAK を実行してゲスト入口まで到達したことも確認します。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-isa-errors.py target/release/penv ../renode "$work/isa-valid" --valid
python3 scripts/validate-rv32-isa-errors.py target/release/penv ../renode "$work/isa-errors"
```

RV32E・V・Andes 拡張を必要とする構築ケースは、それぞれ未実装の機能を要求するため台帳の対象に残しています。混在 RV32/RV64 のケースは既存の範囲判定を維持します。上流等価合格は累計 47 件です。

### RV32 Zicond の固定 Robot ケース

既存 Zicond 実装の RV32 ケース 7 件を、固定上流から実行可能な ELF に移植しました。RV32 REPL の `init:` に `MTVEC VALUE` を追加し、既存の `MSTATUS VALUE` と同じブロックで記述順に適用します。これにより、上流と同じ `0x1010` のトラップ先をゲストの実行前に設定できます。

`validate-rv32-zicond.py` は上流のオペランド、期待値、例外番号、PC を使います。上流の停止中の 2 番目の CPU はアセンブル用なので、移植では同じ命令語を ELF に格納します。命令命中時の結果レジスターと、拡張無効時のトラップ位置・`mcause`・`mepc` を ELF 内で確認します。7 件が通過し、公開 Zicond ELF 4 件および既存の `MSTATUS` 初期化を使う Sm ACT 診断も通過しました。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-zicond.py target/release/penv ../renode "$work/zicond-robot"
```

上流等価合格は累計 54 件です。RV64 専用の同スイートのケースは既存の対象外判定を維持しています。上流最大 ISA と最終分母の確定は残っています。

### RV32 MRET と特権レベルの構成

固定 `tests/unit-tests/riscv-mret.robot` の `User Level Exists 32Bits` と `User Level Does Not Exists 32Bits` を Rust のマシンテストへ移植しました。変更前は `privilegeLevels` と数値の `interruptMode: 1` が拒否され、これらを取り除くと割り込み配線のない CLINT も拒否されました。

`CPU.RiscV32` の REPL は `Machine`、`MachineUser`、`MachineSupervisorUser` と数値の割り込みモードを受け付けます。割り込み配線を省略した CLINT は時刻とレジスターを提供し、出力をハートへ配送しません。マシンテストでは固定 `rv32gc` の REPL、0x2000 の `0xc0001073`、0x4000 の `0x30200073` を使用します。最初のステップは両構成とも PC=0x4000、MPP=M で、次のステップは PC=0x2000、MPP は `MachineUser` で U、`Machine` で M でした。

```bash
cargo test --locked upstream_rv32_mret -- --nocapture
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-interrupt-mode.py target/release/penv ../renode "$evidence/interrupt-mode"
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform ../renode/platforms/cpus/riscv_virt.repl \
  --artifacts "$evidence/privilege" "$work"/isa/rv32[ms]i-*
```

`cargo test` は 77 件、割り込みモードの移植は 12 件、公開 M/S 特権 ELF は 22 件が通過しました。`scripts/validate-riscv-acceptance.sh` と同じ手順で virt の NS16550・CLINT・PLIC プローブと、固定上流の Bare/PMP MRET ELF も確認しました。後者は SHA-1 が一致し、43 ステップで想定 PC 0x10000 に到達しました。元の Robot が検査する関数ログは引き続き未確認のため、台帳では部分移植です。

固定 `riscv-priv.robot` の RV32 フィクスチャは `rv32imafd_zicsr_zifencei_c_v_zba_zbb_zbc_zbs_zfh_zacas_zcb` を要求し、CPU 作成が `c, v, zcb` で止まります。カウンタ 6 件と権限命令 3 件はこの阻害点を台帳に記録しました。縮小 ISA の検証は補助証拠として扱います。今回の上流等価合格は 2 件で累計 56 件です。上流最大 ISA と最終分母の確定が残っています。

ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-850b661-2xKxYz/` に保存しました。

### RV32 の既定 MTVEC と例外レジスター

固定 `riscv-unit-tests.robot` の RV32 5 ケースを移植しました。固定 tlib の `DEFAULT_MTVEC` は 0x1010 です。penv は、同アドレスに 4 バイトの RAM を持つ `CPU.RiscV32` REPL で、ゲスト実行前にその値を設定します。REPL の明示モード、初期化コマンド、ゲストからの CSR 書き込みを引き続き適用します。既定アドレスに RAM を持たないプラットフォームでは、ベクター設定前の CLI 停止動作を使います。この場合のリセット直後の MTVEC は固定 Renode と異なります。

移植は固定の `rv32gc_Zfh` フィクスチャで、3 種類の不正命令・CSR アクセスについて 3 ステップ後の PC、MCAUSE、MTVAL、MEPC を照合します。EBREAK と ECALL は同一マシンで MTVAL を事前設定し、各ステップでクリアと原因を確認します。M モードでの SRET は SEPC 0x1234 への遷移を確認します。

```bash
cargo test --locked upstream_rv32_unit_traps_and_sret -- --nocapture
cargo test --locked
cargo build --locked --release
target/release/penv run --platform "$evidence/cpu.repl" --stats --max-steps 3 "$evidence/fault.elf"
python3 scripts/validate-rv32-interrupt-mode.py target/release/penv ../renode "$evidence/interrupt-mode-rerun"
```

保存した `fault.S` と `fault.ld` は上流の不正命令ケースの命令語と配置を再現します。変更前は PC 0x4004 で終了コード 1、変更後は 0x1010 に遷移し、指定したステップ上限で終了コード 2 でした。テスト 78 件、既存割り込みモード 12 件、公開 M/S 特権 ELF 22 件、virt の UART・CLINT・PLIC プローブと上流 Bare/PMP MRET ELF を確認しました。

台帳の上流等価合格は累計 61 件です。同じスイートの低アドレスのデータアクセス、モニターの x0 書き込みと ResetVector、命令診断ログの検証が残っています。固定 `riscv-priv.robot` の最大構成に残る ISA 阻害点も継続です。

再現用 ELF、ソース、ログと終了状態は `/home/ec2-user/.local/state/penv-dispatch/evidence/riscv/backlog-871700a-6getcP/` に保存しました。

### RV32 の未マップ先への整数ロード・ストア

固定 `riscv-unit-tests.robot` の RV32 メモリー命令 10 ケースを Rust へ移植しました。固定フィクスチャの RAM は 0x1000 から始まり、命令は 0x5、0x6、0x8 のいずれかにアクセスします。変更前は、バイトロードでもアクセス例外で PC が 0x1010 に遷移しました。

標準 `CPU.RiscV32` のみで構成される REPL で、既存の Renode 互換バス処理を有効化しました。未マップ先の通常ロードはゼロを返し、ストアは破棄します。命令フェッチ、PMP、ページ変換、アライメントの検査は引き続き適用します。固定フィクスチャで元の命令語と a1 を使い、1 ステップ後の PC を全件で、MCAUSE をトラップ 4 件で照合しました。

```bash
cargo test --locked upstream_rv32_unit_data_accesses -- --nocapture
cargo test --locked
cargo build --locked --release
clang --target=riscv32-unknown-elf -march=rv32gc -mabi=ilp32 -nostdlib -fuse-ld=lld \
  -Wl,-Ttext,0x2000 "$evidence/access.S" -o "$evidence/access.elf"
target/release/penv run --platform "$evidence/cpu.repl" --stats --max-steps 1 "$evidence/access.elf"
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform ../renode/platforms/cpus/riscv_virt.repl \
  --artifacts "$evidence/privilege-rerun" "$work"/isa/rv32[ms]i-*
bash scripts/validate-riscv-upstream.sh "$(realpath target/release/penv)" \
  "$(realpath ../renode)" "$evidence/litex-rerun"
```

LB の再現 ELF は変更後、1 命令をリタイアして期待 PC 0x2004 に到達しました。テスト 79 件、公開 M/S 特権 ELF 22 件とアライメント済みの ISA ELF 120 件、virt の UART・CLINT・PLIC プローブが通過しました。実ファームウェア回帰スクリプトも完走し、LiteX 単一ハート・SMP Linux、Minerva の UART からのロードと実行、I²S の PCM バイト一致を確認しました。上流等価合格は累計 71 件です。Renode の未マップアクセス診断ログ、モニターの x0・ResetVector、無効命令のログ、V と Zcb が必要な固定特権フィクスチャは残っています。

再現用 ELF、ソース、ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-f146f92-5wB6Ke/` に保存しました。

### RV32 のモニター経由の整数レジスター操作

起動 RESC の `cpu SetRegister INDEX VALUE` と `cpu GetRegister INDEX` で、RV32 の整数レジスター 0–31 を操作できます。MTVEC/STVEC と整数レジスターの操作は記述順に適用します。固定 `riscv-unit-tests.robot` の `Should Fail On Setting X0 register` は、同じハートに対し値 1 の書き込みが読み取り専用エラーになり、直後の x0 読み戻しが 0 であることを Rust のマシンテストで確認しました。

起動 CLI はコマンドエラーで終了するため、同一マシンでのエラー後の読み戻しは Rust テストで確認し、CLI ではエラーと正常系を個別に検証しました。正常系では a1 を 2、0 の順に書き換え、読み戻しを確認してからプローブ ELF を実行し、完了コード 0 を確認しました。固定の x0 ケースには ELF ロードがありません。CLI 検証では、起動 RESC が要求する ELF を添えています。

```bash
cargo test --locked upstream_rv32_monitor_rejects_writes_to_x0
cargo test --locked
cargo build --locked --release
target/release/penv run --script "$evidence/x0.resc"  # 終了コード 1
target/release/penv run --script "$evidence/readback.resc" --tohost 0x3000 --max-steps 10
python3 scripts/validate-rv32-interrupt-mode.py target/release/penv ../renode "$evidence/interrupt-mode-rerun"
```

テスト 80 件、割り込みモード 12 件、公開 M/S 特権 ELF 22 件、virt の UART・CLINT・PLIC プローブが通過しました。上流等価合格は累計 72 件です。ResetVector ケースには、CPU Reset と PC 読み戻しを同一マシン上で順に実行する仕組みが残っています。数値の整数レジスター以外の `GetRegister`/`SetRegister`、実行中の対話操作は今回の対象外です。

再現用 RESC、ELF、ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-b16e5bb-duiIqa/` に保存しました。

### RV32 の ResetVector と CPU Reset

固定 `riscv-unit-tests.robot` の RV32 ResetVector ケースを起動 RESC に移植しました。PC の初期値 0x1000、ResetVector 更新による PC の追従、明示 PC 設定後の追従停止、CPU Reset 後の追従再開を同一マシンで照合します。2 回のリセットを含む PC 読み戻し 8 件がすべて一致しました。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-reset-vector.py target/release/penv "$evidence/robot-rerun"
target/release/penv run --script "$evidence/firmware.resc" --tohost 0x3000 --max-steps 10
```

ELF をロードした追加検証では、ResetVector の変更後にも ELF エントリーを保持し、CPU Reset 後は指定エントリーから実行して完了コード 0 に到達しました。Rust テスト 80 件、公開 M/S 特権 ELF 22 件、割り込みモード 12 件、virt の UART・CLINT・PLIC プローブが通過しました。

今回の CPU Reset は起動時の主ハートを REPL 初期状態に戻します。PC、ResetVector、Reset は互いの順序を維持します。ELF のロードと CSR・整数レジスターの起動設定は別フェーズのため、これらをリセットの前後に混在させる一般的なモニター順序や実行中のリセットは未対応です。上流等価合格は累計 73 件です。

再現用 RESC、ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-24652b2-DJlsCV/` に保存しました。

### RV32 の MRET/TLB ケースにおける関数ログ

固定 `riscv-mret.robot` の `Should Invalidate TLB After Mode Change` を等価検証しました。起動 RESC の `cpu LogFunctionNames true` で、RV32 ELF の実行可能セクションにある関数シンボルとコードラベルを読み取り、該当アドレスの命令フェッチに成功した時点で関数名を出力します。固定 ELF の `unrecoverable` と `fail_loop` は型指定のないローカルラベルです。

固定 ELF の SHA-1 を確認し、Robot と同じ REPL で実行しました。期待ログ `Entering function unrecoverable` が出るまでに `Entering function fail_loop` は出ませんでした。別実行で PC を 0x2018 に設定すると `fail_loop` が出力され、失敗条件を観測できることも確認しました。実行は 44 ステップで区切り、Robot の成功・失敗判定と同じ順序でログを照合しています。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-mret-log.py target/release/penv "$evidence/mret.elf" "$evidence/robot-rerun"
```

Rust テスト 80 件、公開 M/S 特権 ELF 22 件、virt の UART・CLINT・PLIC プローブ、ResetVector 回帰が通過しました。上流等価合格は累計 74 件です。関数ログはシンボルの開始アドレスに対応します。Renode の翻訳ブロック単位のログ、範囲内のシンボル解決、接頭辞フィルターは今回の範囲に含まれません。別ケースの無効命令診断ログ、最大 ISA 構成と未確定の台帳展開は残っています。

固定 ELF、再現用 RESC、ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-04137b1-z3RqND/` に保存しました。

### RV32 Zbkb の固定 Robot ケース

RV32 CPU に `Zbkb` を追加しました。PACK、PACKH、ZIP、UNZIP、BREV8 と、Zbb と共有する論理演算・ローテート・REV8 をデコードします。固定 `riscv-bkb.robot` の RV32 5 ケースは、元の `rv32gc_zbkb` フィクスチャでオペランド、各 1 ステップの非トラップ条件、結果レジスターを照合しました。

```bash
cargo test --locked upstream_rv32_zbkb -- --nocapture
cargo test --locked
cargo build --locked --release
sed '/cpuType:/s/rv32imac/rv32gc_zbkb/' ../renode/platforms/cpus/riscv_virt.repl > "$evidence/zbkb.repl"
python3 scripts/validate-rv32-architecture.py --penv target/release/penv \
  --platform "$evidence/zbkb.repl" --artifacts "$evidence/zbkb-rerun" "$work"/isa/rv32uzbkb-[pv]-*
```

公開 Zbkb ELF は変更前に CPU 作成で 10 件とも停止し、修正後に縮小 ISA 構成で 10 件とも通過しました。既存 Zba・Zbb・Zbc・Zbs の公開 ELF 64 件、virt の UART・CLINT・PLIC プローブ、Rust テスト 81 件も通過しました。公開 ELF の上流共通構成は引き続き V を要求するため、縮小構成の結果は上流等価合格に算入していません。固定 Robot の 5 件を加え、上流等価合格は累計 79 件です。

変更前後のログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-a7ba641-zB4Rkf/` に保存しました。

### RV32 AMOCAS と非ゼロの hartId

固定 `riscv-amocas.robot` の RV32 単一ページ 7 ケースを移植しました。変更前は、RV32 ハートだけを取り出したフィクスチャの `hartId: 2` が REPL の連番制約で拒否されました。設定した ID を `mhartid` に返し、CLINT、割り込み配送、LR/SC 予約は内部のハート番号を使うようにしました。

固定フィクスチャの RV32 ISA、ハート ID、メモリー範囲、アドレス、命令語、レジスターとメモリー初期値を保持しました。対象外の RV64 ハートと 4 GiB 以降の MMIO は、選択したハートとアドレスの単一ステップに関与しないため省略しました。`CyclesPerInstruction` の時間設定も省略し、元ケースが観測する全レジスター・メモリー値と、奇数レジスター対の PC・MCAUSE・MTVAL・MEPC を照合しました。

```bash
cargo test --locked upstream_rv32_amocas_single_page -- --nocapture
cargo test --locked
cargo build --locked --release
target/release/penv run --platform "$evidence/virt-hart32.repl" \
  --tohost 0x80001000 --max-steps 100 "$evidence/hart.elf"
python3 scripts/validate-rv32-architecture.py --act --penv target/release/penv \
  --platform "$evidence/act-hart2.repl" --artifacts "$evidence/act-rerun" \
  "$work"/act/work/spike-rv32-max/elfs/rv32i/Zacas/*.elf
```

固定 Zacas ACT ELF は UART を持たない Robot フィクスチャでは終了処理の送信待ちに入りました。NS16550 を追加した縮小構成では `hartId: 2` で完了コード 0 を確認しました。この ACT 結果は上流最大構成の合格には算入していません。virt の別プローブで `mhartid=32` と LR/SC、既存の実 ELF で CLINT・PLIC・UART を確認しました。Rust テスト 82 件と、LiteX SMP Linux の 4 ハート起動・ログイン・CPU 一覧も通過しました。上流等価合格は累計 86 件です。最大 ISA 構成の V と Zcb、および未確定の台帳展開は残っています。

再現用 REPL、プローブ、ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-266bffd-iiUKGl/` に保存しました。

### RV32E の構成と整数レジスター制約

固定 `riscv-isa-string.robot` で RV32E を要求する 14 表記を移植しました。以前は `rv32emac_zicsr` が CPU 作成時に拒否されました。RV32E では `misa.E` を立て、整数命令で x16–x31 を指定した場合は不正命令とします。整数レジスターを使うフィールドだけを検査し、CSR の即値は 0–31 のままです。圧縮命令は展開後に同じ検査を行い、例外には元の命令語を記録します。ELF ローダーは RV32E 構成で `EF_RISCV_RVE` を受け付けます。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-isa-errors.py target/release/penv ../renode "$evidence/valid-rerun" --valid
python3 scripts/validate-rv32-isa-errors.py target/release/penv ../renode "$evidence/invalid-rerun"
riscv64-unknown-elf-gcc -march=rv32emac_zicsr -mabi=ilp32e -nostdlib \
  -Wl,-T,scripts/firmware/rv32-virt.ld scripts/firmware/rv32e.S -o "$evidence/rv32e.elf"
target/release/penv run --platform "$evidence/rv32e.repl" \
  --tohost 0x80001000 --max-steps 256 "$evidence/rv32e.elf"
```

固定 Robot の有効表記 43 件と無効構成 5 件、Rust テスト 82 件、公開 M/S 特権 ELF 22 件、virt の UART・CLINT・PLIC が通過しました。固定 ACT の取得済みツリーに RV32E ELF は見つからなかったため、実行検証には GCC でビルドした `ilp32e` プローブを使用しました。プローブは `misa`、CSR 即値、不正な x16 の読み書きと圧縮命令の例外を確認して完了コード 0 に到達しました。

上流等価合格は累計 100 件です。V と Andes 拡張を要求する構成ケースは残っています。

REPL、再現 ELF、ログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-10f222a-HmbHR9/` に保存しました。

### RV32 の無効化された V 命令の診断

固定 `riscv-unit-tests.robot` の `Should Exit Translation Block After Invalid Instruction And Report Single Error` を移植しました。変更前は不正命令トラップに入るものの、上流が要求する診断が出ませんでした。V が無効な RV32 で OP-V 命令に到達すると、PC と命令語を含む Renode 形式の診断を出して既存の不正命令トラップに入ります。

検証器は固定 Robot から命令語と書き込み位置を抽出し、同じ REPL で開始 PC から連続実行します。上流の 1 MIPS・10 µs の実行窓は 10 ステップで再現します。診断は最初の 0x2008 で 1 回出力され、後続の 0x2018 と 0x2020 では出力されませんでした。翻訳ブロックや仮想時刻の挙動はこの検証の観測対象に含まれません。

```bash
cargo test --locked
cargo build --locked --release
python3 scripts/validate-rv32-vector-diagnostic.py target/release/penv ../renode "$evidence/robot-rerun"
```

Rust テスト 82 件、公開特権 ELF 22 件、virt の UART・CLINT・PLIC、ベクトル初期化 ELF が通過しました。上流等価合格は累計 101 件です。ベクトル命令本体と、翻訳ブロックの境界をログで検査するタイマーケースは残っています。

変更前後のログと終了状態は `~/.local/state/penv-dispatch/evidence/riscv/backlog-74b8d15-XZOEpw/` に保存しました。
