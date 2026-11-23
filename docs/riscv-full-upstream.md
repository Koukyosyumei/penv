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
