# box

[English](README.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

`git clone --local`でワークスペースを管理するCLIツール。複数リポジトリ対応。

![demo](./demo.gif)

## boxとは？

Boxは`git clone --local`を使って名前付きワークスペースを作成・管理するツールです。

- `box repo add`でリポジトリを登録し、セッションを作成すると`~/.box/workspaces/<session>/`にクローンされる
- 複数リポジトリを1つのセッションにまとめられる
- 各クローンは独立した`.git`を持つので、元リポジトリに影響しない

## 特徴

- **`git clone --local`ワークスペース** — ハードリンクによる高速クローン、独立した`.git`
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
box new <name> --repo <r> [-- cmd...]  新しいセッションを作成
box edit <name>                        セッションのリポジトリを追加・削除
box exec <name> -- <cmd...>            セッションのワークスペースでコマンドを実行
box list [options]                     セッション一覧（エイリアス: ls）
box remove <name>                      セッションとワークスペースを削除
box cd <name>                          セッションのワークスペースにcd
box repo add [path]                    gitリポジトリを登録
box repo remove <name>                 リポジトリの登録を解除
box repo list                          登録リポジトリ一覧（エイリアス: ls）
box config zsh|bash                    シェル設定を出力
box upgrade                            最新版にアップグレード
```

### セッションの作成

```bash
# コマンドを指定して作成
box new my-feature --repo my-app -- make test

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
| `--repo <name>` | クローンするリポジトリ（必須、複数指定可） |
| `-- cmd...` | ワークスペースで実行するコマンド（デフォルト: `$BOX_DEFAULT_CMD`が設定されていればそれを使用） |

### `box list`

| オプション | 説明 |
|--------|-------------|
| `--project`, `-p` | 現在のプロジェクトのセッションのみ表示 |
| `--quiet`, `-q` | セッション名のみ出力 |

## 環境変数

| 変数 | 説明 |
|----------|-------------|
| `BOX_DEFAULT_CMD` | 新規セッションのデフォルトコマンド。`-- cmd`が指定されていない場合に使用 |

## シェル連携

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

タブ補完と、`box cd`で作業ディレクトリを切り替えるための`box`シェル関数が提供されます。

## 仕組み

```
登録済みリポジトリ     box new my-feature        ~/.box/workspaces/my-feature/
  frontend/       ──── git clone --local ────>      frontend/
  backend/                                          backend/
```

`git clone --local`はハードリンクを使って独立したgitリポジトリを作成します。各クローンは独自の`.git`ディレクトリを持つため、ワークスペース内でのコミット、ブランチ操作、リセットなどが元のリポジトリに影響することはありません。

| 項目 | 詳細 |
|--------|--------|
| ワークスペースの場所 | `~/.box/workspaces/<session>/` |
| セッションメタデータ | `~/.box/sessions/<session>/` |
| クローン方法 | `git clone --local`（独立した`.git`、ハードリンクされたオブジェクト） |
| クリーンアップ | `box remove`でワークスペースとセッションデータを削除 |

## 設計上の判断

<details>
<summary><strong>なぜ <code>git clone --local</code>？</strong></summary>

| 代替手段 | トレードオフ |
|----------|-------------|
| **バインドマウント** | 隔離なし — 変更が元のファイルに直接影響 |
| **git worktree** | `.git`をホストと共有。checkout/reset/rebaseがホストのrefに影響しうる |
| **bare-gitマウント** | 状態を共有。ブランチ操作がホストに影響 |
| **完全コピー（`cp -r`）** | 独立するが大きなリポジトリでは遅い |

`git clone --local`は独立（独自の`.git`）、高速（ハードリンク）、完全（全履歴）、シンプル（ラッパースクリプト不要）です。

</details>

## Claude Code連携

Boxは[Claude Code](https://docs.anthropic.com/en/docs/claude-code)と組み合わせて、独立したワークスペースでAIエージェントを実行できます：

```bash
box new ai-experiment --repo my-app -- claude
```

エージェントの操作はすべてワークスペース内に留まります。完了したらセッションを削除するだけです。

## ライセンス

MIT
