# Changelog

このプロジェクトの主要な変更を記録します。

フォーマットは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に基づいており、
このプロジェクトは [セマンティックバージョニング](https://semver.org/lang/ja/) に準拠しています。

## [Unreleased]

## [0.12.0] - 2026-07-26

### 追加

- **codegen**: channel の **event** に対しても discriminated-union envelope
  (`{Channel}EventEnvelope`) を生成するようにした。従来 `envelope="<tag>"` は
  **request にしか** union を出さず、event は payload 型だけが生成されて
  受け手が判別に使える型が無かった（受信側が手書きで match するしかなく、
  envelope を置く意味が失われていた）。rust / typescript / zod の 3 emitter が対象
  （surrealql には envelope の概念が無い）。
  request 側の `{Channel}Envelope` は名前も内容も不変で、envelope 生成は
  **opt-in のまま**（tag 未宣言の channel は event があっても何も増えない）。PR #33

### 変更

- **MSRV: 1.94 → 1.95**。依存の `kdl` が 6.6.0 で MSRV を 1.82 → 1.95 に
  引き上げたため、宣言 MSRV では **そもそもビルドできない**状態になっていた
  （`Cargo.lock` は untracked なので CI は毎回解決し直し、6.6+ を引く）。
  1.94 を維持するには `kdl` を 6.5 に固定して upstream の修正を止める必要があり、
  実態に合わせて宣言側を上げる判断。PR #33


## [0.11.1] - 2026-05-25

### 修正

- **CI/release**: `release.yml` の crates.io publish 順を依存グラフ topological
  に修正 — `derive → main → compose → codegen` の順に並べ替え。PR #26
  で codegen が compose に依存するようになったが release.yml の順は古い
  まま放置していたため、v0.11.0 release で codegen publish 時点で
  `club-kdl-compose = "^0.11.0"` を crates.io 上で解決できず**部分公開**
  状態（derive と main のみ 0.11.0 published、codegen と compose は
  0.10.0 のまま）になっていた。本リリースで 4 crate 全部を 0.11.1 で
  揃え直す。

### 含まれる機能

- v0.11.0 の機能内容（codegen が compose 経由で multi-file schema 対応 / 新公開
  API `parse_doc` / `parse_path` / `ParseError` の `#[non_exhaustive]` 化）
  は変わらず。本リリースは workflow ordering bug の fix のみ。

## [0.11.0] - 2026-05-25

### 追加

- **codegen: `(<)` directive を透過的に解決** — codegen の CLI と library
  公開 API が `club-kdl-compose` 経由で動作するようになり、`(<)file` /
  `(<)glob` directive を含む multi-file schema を「読んで合成してから
  lower」する流れが標準に。`(<)` を含まない schema は compose を no-op で
  通過し、出力は v0.10.0 と byte 一致（後方互換）。
- **codegen: 新しい公開 API 2 つ**:
  - `parser::parse_doc(&KdlDocument) -> Result<Schema, ParseError>` —
    既に手元に `KdlDocument` がある consumer 向けの IO 無し低レベル API。
  - `parser::parse_path(impl AsRef<Path>) -> Result<Schema, ParseError>` —
    ファイルパスから compose 経由でロード・解決する高レベル API。CLI も
    このパスを通る。
- 既存 `parser::parse(&str)` は不変、引き続き pure-text 入力 path として
  維持される。

### 変更 (BREAKING)

- **codegen: `ParseError` に `#[non_exhaustive]` 属性 + `Compose(ComposeError)`
  variant を追加** — 将来の variant 追加 (Io / Network / 新 dialect の
  validation 等) を non-breaking で行えるようにするための一度きりの調整。
  下流 consumer が `match err { Kdl(_) | Validation(_) => ... }` で網羅
  していた場合は `_ => ...` arm を足す必要がある。

## [0.10.0] - 2026-05-25

### 追加

- **`club-kdl-compose` crate を新設** — KDL ドキュメントの複数ファイル合成を
  提供する独立 crate（bare lib 名 `kdl_compose`）。`(<)file` / `(<)glob`
  directive を解決して指定ファイルの top-level node を取り込み位置に
  splice する。
  - **directive 構文** — タグ `(<)` + variant (`file` / `glob`) の二段構造。
    KDL の `()` タグ慣用「型表明」と区別するため、単語ではなく**記号タグ**を
    採用（`<` の mnemonic は「他ファイルからこの位置に内容が流入する」）。
  - **公開 API**: `compose(path) -> KdlDocument` で解決済みドキュメントを返し、
    `from_path<T>(path) -> T` で `club-kdl` 連携の typed deserialize まで一気通貫。
  - **位置自由** — directive は top-level だけでなく任意の `protocol` / `channel`
    等の block 内にも書ける（block の children として展開）。
  - **取り込み元相対パス** のみ、CWD / search path / 環境変数解決はしない。
  - **cycle 検出** — A→B→A も A→A 自己 include も明示エラーで止める。
  - **glob 0 件マッチ** はエラーではなく空（仕様）。
  - **Phase 2 オプション**（children block 形）: `only "A" "B"` / `except "X"` /
    `rename "Old" "New"` + scalar property `as="ns"` で namespace prefix。
    apply 順は **filter → rename → as= prefix**。`as=` は top-level node の
    first string arg のみ rewrite し、内部参照は author の責務とする
    （compose は schema semantics 非依存）。
  - core `club-kdl` は **pure parser のまま** — `from_str` は IO ゼロを維持し、
    composer は opt-in（`tokio` / `tokio-util` パターン）。
  - **64 テスト**: unit 38 + integration 25 + runnable doc-test 1。
    `transform_node` / `parse_directive` / `parse_options_block` を unit で
    直接叩く設計で、ピラミッドを Small 59 / Medium 39 / Large 2 % に整えた。

## [0.9.1] - 2026-05-20

### 修正

- **codegen: TypeScript の channel type map を `interface` → `type` に** —
  `<Channel>ChannelEventTypes` / `<Channel>ChannelRequestTypes` を `interface`
  で出力していたため、SDK の `ChannelMeta.__types`（`Record<string, unknown>`）
  に代入できなかった（`interface` は暗黙の index signature を持たない）。
  `type` 別名で出力し、生成 `ChannelMeta` const が `ChannelMeta` を満たすようにする。

## [0.9.0] - 2026-05-19

### 追加

- **codegen: protocol 方言に channel envelope enum** — `channel` に
  `envelope="<tag>"` を付けると、その channel の全 `request` を束ねる
  discriminated union を生成する。Rust は `#[serde(tag="…")]` の内部タグ
  enum（field 無し request は unit variant）、TypeScript は
  `({ t: "…" } & Payload)` の判別 union、Zod は `z.discriminatedUnion`。
  `envelope` 無しの channel は従来どおり per-request の型のみで、出力は
  v0.8.0 と byte 一致（後方互換）。

### 変更

- **codegen: identifier sanitize** — `request` / `event` 名に含まれる
  `:` `/` `.` を識別子の語区切りとして扱い、wire 名（`lane:delete`）を
  正当な型名（`LaneDelete`）へ変換する。元の wire 名は `#[serde(rename)]`
  で保持。これまで `:` 入りの名前は生成コードをコンパイル不能にしていた。
  これに伴い protocol 方言の request / event 由来の型名は PascalCase に
  揃う。

### 内部

- 生成出力を実コンパイルする integration test を追加（Rust は使い捨て
  crate で `cargo build`、TypeScript は `bunx tsc --noEmit`）。文字列
  アサーションでは取りこぼす serde 属性の構文ミス等を検出する。CI の
  `test` job に Bun セットアップを追加。

## [0.8.0] - 2026-05-17

### 追加

- **codegen: KDL spec v2 Tier 1** — `record`（実体テーブル）/ `relation`
  （グラフエッジ）/ `link<T>`（レコード参照）/ union 型 / literal 型を追加。
  「構造 + 繋がり」を持つデータモデルを KDL で第一級表現できる。
- **codegen: KDL spec v2 Tier 2** — `field` および型定義の `description`
  （→ doc コメント / `COMMENT` / `.describe()`）、`constraints`
  （min / max / min_length / max_length / pattern → Zod `.min()` 等・
  SurrealQL `ASSERT`）。

### 変更

- **codegen (BREAKING): `field` のデフォルトを required に反転** — 無印の
  `field` は **required**。optional は `optional=#true` で opt-in
  （従来は optional がデフォルト）。
- **codegen: SurrealQL** — `ASSERT … IN` を `INSIDE` に統一。optional
  field の `ASSERT` は `$value = NONE OR …` でガード。

## [0.7.0] - 2026-05-17

### 追加

- **codegen: Zod emitter** — KDL schema から Zod schema (TypeScript の
  ランタイム validator) を生成。 enum は `z.enum`、 struct は `z.object`。
  Zod schema は値で前方参照できないため enum を struct より先に出力する。
  unison のブラウザクライアントが KDL schema 由来の型 + 検証を持つための
  基盤。
- **codegen: SurrealQL emitter** — KDL schema から SurrealDB の schema DDL
  (`DEFINE TABLE` / `DEFINE FIELD`) を生成。 enum 参照は `ASSERT $value IN
  [...]`、 struct 参照は `record<table>` link、 optional は `option<T>`。
  protocol 方言は DB 表現を持たないため data 方言のみが対象。
- **codegen: CLI に `zod` / `surrealql` ターゲット追加** — これで 4 ターゲット
  (rust / typescript / zod / surrealql) が出揃った。
- **codegen**: CLI 統合テストと parse→emit の end-to-end テストを追加
  (`codegen/tests/integration.rs`)。

### 修正

- **codegen**: parser が未定義の型参照 (`field type="..."` が `struct` /
  `enum` として未定義の名前を指す) を検証するように。 これまでは未定義型が
  emitter まで素通りし、 生成コードのコンパイルエラーとして遠くで顕在化
  していた。
- **codegen**: Rust 予約語 (`type` など) の field 名を raw identifier
  (`r#type`) でエスケープ。 生成 Rust コードのコンパイル不能を防ぐ。

### ドキュメント

- **codegen**: `lib.rs` の対応ターゲット記述を実態に同期 (4 ターゲット)。

## [0.6.0] - 2026-05-17

### 追加

- **`club-kdl-codegen` crate を新設**: KDL schema ファイルから Rust /
  TypeScript のコードを生成する crate。 IR (data / protocol の 2 方言) +
  parser + emitter + CLI で構成。 workspace の 3 つ目の member。 (PR #12)
- **NDKDL — `append_node`**: 値を 1 つの KDL ノードとしてファイル末尾に追記する
  ヘルパー (`club_kdl::append_node`)。 KDL を 1 ノード = 1 レコードのストリーム
  (ログ・メトリクス・イベント) として扱うための入口。 `to_string_pretty` が
  ドキュメント全体の round-trip 用なのに対し、 ファイル全体を読まずに追記する。
  追記後のファイルは `#[kdl(document)]` 構造体で読み戻せる。 (Refs #4)
- **guide ドキュメント** (`docs/guide/`): カスタム型ガイド / KDL 設計ベスト
  プラクティス / トラブルシュート の 3 本を新設。
- **README en/ja 構成**: `README.md` (日本語) + `README.en.md` (英語) の
  二言語構成に。

### Note

`append_node` は public API への additive な追加です (semver minor)。

## [0.5.1] - 2026-05-16

### 追加

- **dual license**: `MIT OR Apache-2.0` (Rust ecosystem 標準)
- **MSRV** を明示: `rust-version = "1.94"` (workspace)
- **`[package.metadata.docs.rs]`**: docs.rs での all-features build
- **doctest 実行可能化**: `lib.rs` / `de.rs` / `ser.rs` の example を `ignore` から実行可能 doctest へ
- **CI 強化** (`.github/workflows/ci.yml`):
  - `fmt --check` / `clippy -D warnings` / `doc -D warnings`
  - multi-OS (ubuntu / macOS / windows) × multi-toolchain (stable / beta)
  - MSRV テスト (`rust-version` 自動読み取り)
  - `cargo-deny` (license / advisories / bans)
  - `cargo-semver-checks` (PR 時のみ)
- **Release workflow** (`.github/workflows/release.yml`):
  - tag push (`v*.*.*`) で derive → main の順に自動 publish
  - `workflow_dispatch` で dry-run サポート
  - 自動 GitHub Release 生成
- **OSS hygiene**:
  - `CONTRIBUTING.md` / `SECURITY.md` / `CODE_OF_CONDUCT.md`
  - Issue templates (bug / feature)、 PR template
  - `dependabot.yml` (cargo + github-actions weekly)
  - `deny.toml`
- **derive crate metadata** 補完: `keywords` / `categories` / `readme` / `homepage`

### 変更

- LICENSE 年度更新: `2025` → `2025-2026`
- `Cargo.toml` の derive 依存を `=0.5.1` で pin

### 修正

- `cargo fmt --check` で検出された tests/exhaustive_mapping.rs の use 順序
- `cargo clippy` で検出された警告 9 件:
  - `collapsible_if` (derive/src/lib.rs, 4 件)
  - `bool_assert_comparison` (tests/exhaustive_mapping.rs, 3 件)
  - `approx_constant` (tests/exhaustive_mapping.rs, 2 件)
- benches/kdl_vs_json.rs の unused import / dead code

### Note

これは **品質整備リリース** で、 public API への変更はありません (semver patch)。

## [0.5.0] - 2026-05-15

### 変更 (Breaking)

- lib name を package name と統一: `unison_kdl` → **`club_kdl`** / `unison_kdl_derive` → **`club_kdl_derive`**
- v0.4.0 の rename trick (`[lib].name` 据置 + 内部 dep alias) を撤廃、 命名を一貫させた
- 下流の `use unison_kdl::...` は **`use club_kdl::...`** に書き換えが必要

```toml
# Cargo.toml
club-kdl = "0.5"
```

```rust
use club_kdl::{KdlDeserialize, KdlSerialize};
```

## [0.4.0] - 2026-05-15

### 変更 (Breaking — Cargo.toml level only)

- **crate を `unison-kdl` から `club-kdl` に rename** (chronista-club 命名規則に統一)
  - crates.io 上の名前: `unison-kdl` → **`club-kdl`**
  - derive crate も同様: `unison-kdl-derive` → **`club-kdl-derive`**
  - lib name は `unison_kdl` / `unison_kdl_derive` で据置 — **ソースコードの `use unison_kdl::...` は変更不要**
  - 下流 consumer は Cargo.toml の dep 行のみ更新:

    ```toml
    # 旧
    unison-kdl = "0.3"

    # 新
    club-kdl = "0.4"

    # または alias で旧来の import 感覚を維持
    unison_kdl = { package = "club-kdl", version = "0.4" }
    ```

### 内部

- ディレクトリ構造は据置 (`derive/` 等)。 package name のみ rename。
- `[lib].name` を明示的に指定 (`unison_kdl` / `unison_kdl_derive`) して import path を保護。
- 親 crate の `pub use unison_kdl_derive::...` は alias 付き dep 経由で維持
  (`unison_kdl_derive = { package = "club-kdl-derive", ... }`)。

### 命名規則の根拠

chronista-club ecosystem の crates.io 公開 crate は **`club-` prefix** で統一する。

| Layer | Prefix | 例 |
|-------|--------|----|
| 内部ツール / plugin | `cc-` | `ccwire`, `ccws` |
| 公開 crate (library) | `club-` | `club-unison`, `club-kdl` |

- 関連 PR: chronista-club/unison **PR #31** ([`club-unison` への rename](https://github.com/chronista-club/unison/pull/31))
- 命名規則 memory: creo-memories `mem_1Cb2haX6ZicuCweEpxAvj4`

## [0.3.0] - 2026-03-11

### 追加

- enum data variants 対応 (struct / newtype / unit バリアントの KDL シリアライズ・デシリアライズ)

## [0.2.0] - 2026-03-11

### 追加

- `kdl_node_name()` 自動解決
- `#[kdl(alias = "...")]` 属性
- `usize` 型対応
- 網羅テスト整備

[Unreleased]: https://github.com/chronista-club/club-kdl/compare/v0.11.1...HEAD
[0.11.1]: https://github.com/chronista-club/club-kdl/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/chronista-club/club-kdl/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/chronista-club/club-kdl/compare/v0.9.0...v0.10.0
[0.9.1]: https://github.com/chronista-club/club-kdl/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/chronista-club/club-kdl/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/chronista-club/club-kdl/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/chronista-club/club-kdl/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/chronista-club/club-kdl/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/chronista-club/club-kdl/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/chronista-club/club-kdl/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/chronista-club/club-kdl/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/chronista-club/club-kdl/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/chronista-club/club-kdl/releases/tag/v0.2.0
