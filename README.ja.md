# box

[English](README.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

サンドボックス化されたgitワークスペースを管理するCLIツール。複数リポジトリ対応。

![demo](./demo.gif)

## boxとは？

Boxは`git worktree`（デフォルト）または`git clone --local`を使って名前付きワークスペースを作成・管理するツールです。

- `box repo add`でリポジトリを登録し、セッションを作成すると`~/.box/workspaces/<session>/`にワークスペースが作られる
- 複数リポジトリを1つのセッションにまとめられる
- worktreeモードは軽量で高速、cloneモードは完全な`.git`分離を提供

## 特徴

- **2つのワークスペース戦略** — `git worktree`（デフォルト、軽量）または`git clone --local`（完全分離）
- **マルチリポセッション** — 複数リポジトリを1つのワークスペースにまとめる
- **インタラクティブTUI** — リポジトリ選択、セッション名・コマンド入力、履歴対応
- **シェル連携** — zsh/bashの補完と`cd`ラッパー

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
# 1. リポジトリを登録
box repo add ~/projects/my-app

# 2. TUIでセッション作成
box

# 3. またはCLIで作成
box new my-feature --repo my-app -- make test

# 4. クリーンアップ
box remove my-feature
```

## 使い方

```bash
box                                    インタラクティブTUI（新規セッション作成）
box new <name> --repo <r> [options]     新しいセッションを作成
box edit <name>                        セッションのリポジトリを追加・削除
box list [options]                     セッション一覧（エイリアス: ls）
box remove [<name>]                    セッションとワークスペースを削除（エイリアス: rm）
box cd <name>                          セッションのワークスペースにcd
box repo add [path]                    gitリポジトリを登録
box repo remove <name>                 リポジトリの登録を解除（エイリアス: rm）
box repo list                          登録リポジトリ一覧（エイリアス: ls）
box repo update [options]              登録リポジトリをfetch & pull
box config zsh|bash                    シェル設定を出力
box upgrade                            最新版にアップグレード
```

### セッションの作成

```bash
# コマンドを指定して作成（デフォルトはworktree）
box new my-feature --repo my-app -- make test

# clone戦略で完全分離
box new my-feature --repo my-app --strategy clone

# 複数リポジトリ
box new my-feature --repo frontend --repo backend

# 最小限
box new my-feature --repo my-app
```

`--repo`は必須です。対話的にセッションを作成するには、引数なしで`box`を実行してTUIを起動してください。

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
box remove my-feature           # セッションとワークスペースを削除
```

### ワークスペースへの移動

```bash
box cd my-feature               # セッションのワークスペースにcd
```

シェル連携を有効にしている場合（`eval "$(box config zsh)"`）、`box cd`で作業ディレクトリが切り替わります。未設定の場合はワークスペースのパスが標準出力に表示されます。

## マルチリポワークスペース

リポジトリを登録し、セッション作成時に名前で指定します：

```bash
box repo add ~/projects/frontend
box repo add ~/projects/backend

box new my-feature --repo frontend --repo backend
```

各リポジトリは`~/.box/workspaces/<session>/<repo>/`にクローンされます。単一リポジトリの場合、ワークスペースパスはリポジトリのサブディレクトリに直接解決されます。

## オプション

### `box new`

| オプション | 説明 |
|--------|-------------|
| `<name>` | セッション名（必須） |
| `--repo <name>` | 含めるリポジトリ（必須、複数指定可） |
| `--strategy <strategy>` | `worktree`（デフォルト）または`clone` |

### `box list`

| オプション | 説明 |
|--------|-------------|
| `--project`, `-p` | 現在のプロジェクトのセッションのみ表示 |
| `--quiet`, `-q` | セッション名のみ出力 |

### `box repo update`

| オプション | 説明 |
|--------|-------------|
| `--all`, `-a` | 対話選択なしで全登録リポジトリをupdate |
| `--force`, `-f` | update前に未コミットの変更をstash |

## 環境変数

| 変数 | 説明 |
|----------|-------------|
| `BOX_STRATEGY` | デフォルトのワークスペース戦略（`worktree`または`clone`）。`--strategy`フラグで上書き可能 |

## シェル連携

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

タブ補完と、`box cd`で作業ディレクトリを切り替えるための`box`シェル関数が提供されます。

## 仕組み

Boxは2つのワークスペース戦略をサポートしています：

### Worktree（デフォルト）

```
登録済みリポジトリ     box new my-feature        ~/.box/workspaces/my-feature/
  frontend/       ──── git worktree add ─────>      frontend/
  backend/                                          backend/
```

`git worktree`は元のリポジトリにリンクされた軽量な作業ツリーを作成します。オブジェクトストアを共有するため、作成は即座に完了しディスク使用量も最小限です。各worktreeは独自のブランチを持ち、`box remove`でworktreeを適切にクリーンアップします。

### Clone

```
登録済みリポジトリ     box new my-feature        ~/.box/workspaces/my-feature/
  frontend/       ──── git clone --local ────>      frontend/
  backend/                                          backend/
```

`git clone --local`はハードリンクを使って完全に独立したgitリポジトリを作成します。各クローンは独自の`.git`ディレクトリを持つため、ワークスペース内でのコミット、ブランチ操作、リセットなどが元のリポジトリに影響することはありません。

### 比較

| | Worktree | Clone |
|---|---|---|
| 速度 | 即座 | 高速（ハードリンク） |
| ディスク使用量 | 最小（オブジェクト共有） | 少ない（ハードリンク） |
| 分離性 | 作業ツリーは分離、`.git`は共有 | 完全に独立した`.git` |
| 適した用途 | フィーチャーブランチ、実験 | 完全分離、破壊的操作 |

| 項目 | 詳細 |
|--------|--------|
| ワークスペースの場所 | `~/.box/workspaces/<session>/` |
| セッションメタデータ | `~/.box/sessions/<session>/` |
| デフォルト戦略 | `git worktree`（`--strategy clone`で変更可能） |
| クリーンアップ | `box remove`でワークスペースとセッションデータを削除 |

## ライセンス

MIT
