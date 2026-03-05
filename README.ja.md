# box

[English](README.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

開発用の隔離されたgitワークスペース。クローンして、ブランチして、壊しても — 元のリポジトリは無傷。

![demo](./demo.gif)

## なぜ box？

Boxは**隔離された**gitワークスペースと**セッション管理**を提供します — 元のリポジトリに触れることなく自由に実験できます。

各セッションは独自のワークスペースを取得します。デフォルトでは `git clone --local` がハードリンクを使って完全に独立したリポジトリを作成します — 大きなリポジトリでも高速で、何をしても元のリポジトリには影響しません。また、`--strategy worktree` を使えば `git worktree` によるさらに高速で省スペースなワークスペースも利用できます。

## 特徴

- **隔離されたgitワークスペース** — `git clone --local`（デフォルト）または `git worktree` でセッションごとにワークスペースを作成。ホストのファイルは変更されない
- **セッション管理** — プロジェクトディレクトリ、コマンド、戦略、作成日時をメタデータで追跡
- **マルチリポジトリワークスペース** — リポジトリを登録し、複数リポジトリにまたがるワークスペースを作成
- **シェル連携** — ワークスペースへの自動cd

## 必要なもの

- [Git](https://git-scm.com/)

## インストール

### クイックインストール

```bash
curl -fsSL https://raw.githubusercontent.com/yusukeshib/box/main/install.sh | bash
```

### crates.ioから

```bash
cargo install box-cli
```

### ソースから

```bash
cargo install --git https://github.com/yusukeshib/box
```

### Nix

```bash
nix run github:yusukeshib/box
```

### バイナリダウンロード

ビルド済みバイナリは[GitHub Releases](https://github.com/yusukeshib/box/releases)ページからダウンロードできます。

## クイックスタート

```bash
box new my-feature
# 隔離されたgitワークスペースを作成し、その中にcdする
```

Boxはgitリポジトリ内で実行する必要があります。現在のリポジトリを `~/.box/workspaces/<name>/` にクローンします。

```bash
# 隔離されたワークスペースで作業...
$ git checkout -b experiment
$ make test  # 自由に壊してOK

# 完了？クリーンアップ
box remove my-feature
```

引数なしで `box` を実行するとインタラクティブTUIが起動します。セッションがある場合は選択して再開、ない場合は作成プロンプトが表示されます。

## セッション名

各セッションはシンプルな名前を持ちます：

```bash
box new my-feature
box new my-feature -- make test
```

## 使い方

```bash
box                                    インタラクティブTUIセッションマネージャ
box new [name] [options] [-- cmd...]   新しいセッションを作成
box edit <name>                        セッションのリポジトリを追加・削除
box exec <name> -- <cmd...>            セッションでコマンドを実行
box list [options]                     セッション一覧を表示（エイリアス: ls）
box remove <name>                      セッションまたはワークスペースを削除
box cd <name>                          ホストのプロジェクトディレクトリを表示
box repo add [path]                    gitリポジトリを登録
box repo remove <name>                 リポジトリの登録を解除
box repo list                          登録リポジトリ一覧（エイリアス: ls）
box config zsh|bash                    シェル補完を出力
box upgrade                            最新版にアップグレード
```

### セッションの作成

```bash
# 対話型プロンプト（名前、コマンドを入力）
box new

# コマンドを指定して作成
box new my-feature -- make test

# マルチリポジトリワークスペース（特定のリポジトリを選択）
box new my-feature --repo app-a --repo app-b

# cloneの代わりにgit worktreeを使用（より高速、オブジェクトストアを共有）
box new my-feature --strategy worktree
BOX_STRATEGY=worktree box new my-feature
```

### セッションのリポジトリを編集

```bash
box edit my-feature             # 既存セッションのリポジトリを追加・削除
```

### セッションの一覧と管理

```bash
box list                        # 全セッションを一覧表示
box ls                          # エイリアス
box list -q                     # 名前のみ（スクリプト用途）
box list -p                     # 現在のプロジェクトのセッションのみ
box remove my-feature           # セッション、ワークスペース、データを削除
```

### ワークスペース間のナビゲーション

```bash
box cd my-feature               # ホストプロジェクトディレクトリを表示
```

## オプション

### `box new`

| オプション | 説明 |
|--------|-------------|
| `--strategy <strategy>` | ワークスペース戦略: `clone`（デフォルト）または `worktree`。`$BOX_STRATEGY` を上書き |
| `--repo <name>` | 特定のリポジトリを選択（複数指定可）。登録済みリポジトリのみ |
| `-- cmd...` | 実行するコマンド（デフォルト: `$BOX_DEFAULT_CMD` が設定されている場合はそれを使用） |

### `box list`

| オプション | 説明 |
|--------|-------------|
| `--project`, `-p` | 現在のプロジェクトディレクトリのセッションのみ表示 |
| `--quiet`, `-q` | セッション名のみ出力（スクリプト用途に便利） |

## 環境変数

| 変数 | 説明 |
|----------|-------------|
| `BOX_DEFAULT_CMD` | 新規セッションのデフォルトコマンド。`-- cmd` が指定されていない場合に使用 |
| `BOX_STRATEGY` | ワークスペース戦略: `clone`（デフォルト）または `worktree` |

## シェル補完

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

## 仕組み

```
your-repo/          box new my-feature            ~/.box/workspaces/my-feature/
  .git/        ──── git clone --local ────>         .git/  (独立)
  src/                                              src/   (ハードリンク)
  ...                                               ...
```

デフォルトでは、`git clone --local` がハードリンクを使って完全に独立したgitリポジトリを作成します。クローンは独自の `.git` ディレクトリを持つため、ワークスペース内でのコミット、ブランチ操作、リセット、破壊的操作が元のリポジトリに影響することはありません。

`--strategy worktree` を指定すると、代わりに `git worktree add --detach` を使用します。親リポジトリとオブジェクトストアを共有するため、ワークスペースの作成がより高速で省スペースになります。トレードオフとして、worktreeは親リポジトリとrefを共有します — 完全なgit隔離よりも軽量なワークスペースが必要な場合に使用してください。

| 項目 | 詳細 |
|--------|--------|
| ワークスペースの場所 | `~/.box/workspaces/<name>/` |
| セッションメタデータ | `~/.box/sessions/<name>/` |
| Git隔離 | `clone`（デフォルト）で完全隔離、`worktree` ではオブジェクトストアを共有 |
| クリーンアップ | `box remove` でワークスペースとセッションデータを削除 |

## 設計上の判断

<details>
<summary><strong>なぜ <code>git clone --local</code> がデフォルト？</strong></summary>

| 戦略 | トレードオフ |
|------|-------------|
| **ホストリポジトリをバインドマウント** | 隔離なし — 変更が実際のファイルに直接影響する |
| **git worktree** | `.git` ディレクトリをホストと共有するため、checkout・reset・rebaseがホストのブランチやrefに影響する |
| **bare-gitマウント** | 状態を共有するため、ブランチ作成・削除がホストに影響する |
| **ブランチのみの隔離** | 共有refに対する破壊的なgitコマンドを防げない |
| **完全コピー（`cp -r`）** | 完全に隔離されるが、大きなリポジトリでは遅い |

`git clone --local` は完全に独立（独自の `.git`）、高速（ハードリンク）、完全（全履歴）、シンプル（ラッパースクリプト不要）です。

なお、速度やディスク節約が完全な隔離よりも重要な場合には、`--strategy worktree` で `git worktree` を利用できます。

</details>

## Claude Code連携

Boxは[Claude Code](https://docs.anthropic.com/en/docs/claude-code)と組み合わせて、隔離されたワークスペースでAIエージェントを実行するのに適しています：

```bash
box new ai-experiment -- claude
```

エージェントが行うすべての操作はワークスペース内に留まります。完了したらセッションを削除すれば消えます。

## ライセンス

MIT
