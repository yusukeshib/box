# box

[English](README.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

サンドボックス化されたgitワークスペースを管理するCLIツール。複数リポジトリ対応。

![demo](./demo.gif)

## boxとは？

Boxは`git worktree`（デフォルト）または`git clone --local`を使って名前付きワークスペースを作成・管理するツールです。

- `box repo add`でリポジトリを登録（`~/.box/repos/`にbare clone）し、セッションを作成すると`~/.box/workspaces/<session>/`にワークスペースが作られる
- 複数リポジトリを1つのセッションにまとめたり、再利用できる**プリセット**として保存できる
- worktreeモードは軽量で高速、cloneモードは完全な`.git`分離を提供

## 特徴

- **2つのワークスペース戦略** — `git worktree`（デフォルト、軽量）または`git clone --local`（完全分離）
- **マルチリポセッション** — 複数リポジトリを1つのワークスペースにまとめる。プリセットで再利用可
- **インタラクティブTUI** — リポジトリ選択、セッション名入力、履歴対応
- **シェル連携** — zsh/bashの補完、`cd`ラッパー、ターミナルタブ名のリネーム

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
box new my-feature --repo my-app

# 4. あとからワークスペースに切り替え
box switch my-feature

# 5. クリーンアップ
box remove my-feature
```

## 使い方

```bash
box                                        インタラクティブTUI（新規セッション作成）
box new <name> --repo <r> [options]        新しいセッションを作成
box edit <name> [--add <r>] [--remove <r>] セッションのリポジトリを追加・削除
box list [options]                         セッション一覧（エイリアス: ls）
box remove [<name>] [--all]                セッションを削除（エイリアス: rm）
box switch <name>                          セッションのワークスペースに切り替え（エイリアス: cd, sw）
box rebase <branch>                        originをfetchしてHEADを<branch>にrebase
box repo add [path]                        gitリポジトリを登録（bare clone）
box repo remove <name>                     リポジトリの登録を解除（エイリアス: rm）
box repo list                              登録リポジトリ一覧（エイリアス: ls）
box preset add <name> --repo <r>...        プリセットを作成または更新
box preset edit <name>                     プリセットのリポジトリを編集（TUI）
box preset remove <name>                   プリセットを削除（エイリアス: rm）
box preset list                            プリセット一覧（エイリアス: ls）
box config zsh|bash                        シェル設定を出力
box upgrade                                最新版にアップグレード
```

`-v`/`--verbose`はグローバルフラグ（`BOX_VERBOSE=1`でも有効化可能）で、詳細な出力を有効にします。

### セッションの作成

```bash
# デフォルト（worktree戦略）
box new my-feature --repo my-app

# clone戦略で完全分離
box new my-feature --repo my-app --strategy clone

# 複数リポジトリ
box new my-feature --repo frontend --repo backend

# プリセットから作成
box new my-feature --preset work
```

`--repo`（複数指定可）または`--preset`が必須です。対話的にセッションを作成するには、引数なしで`box`を実行してください。

### セッションのリポジトリを編集

```bash
box edit my-feature                                # TUI: リポジトリのトグル
box edit my-feature --add app-c                    # 非対話的に追加
box edit my-feature --add app-c --remove app-a     # 追加と削除
```

`--add`と`--remove`は複数指定可能です。どちらか指定するとTUIをスキップします。

### セッションの一覧と管理

```bash
box list                        # 全セッションを一覧表示
box ls                          # エイリアス
box list -q                     # 名前のみ（スクリプト用途）
box list -p                     # 現在のプロジェクトのセッションのみ
box remove my-feature           # 名前で削除
box remove                      # 対話的セレクター（複数選択可）
box remove --all                # すべてのセッションを削除
```

### ワークスペースへの切り替え

```bash
box switch my-feature           # セッションのワークスペースに切り替え
box cd my-feature               # エイリアス
box sw my-feature               # エイリアス
```

シェル連携を有効にしている場合（`eval "$(box config zsh)"`）、`box switch`で作業ディレクトリが切り替わります。`BOX_POST_SWITCH_HOOK`を設定しておくと、セッション名を引数にそのフックも実行されます（[シェル連携](#シェル連携)参照）。シェル連携未設定の場合はワークスペースのパスが標準出力に表示されます。

### 現在のブランチをrebase

```bash
box rebase main                 # bare repoでoriginをfetchし、HEADをmainにrebase
```

`box rebase`はセッションのworktree内から実行します。worktreeの背後にあるbare repoをfetch（兄弟worktreeのブランチも安全に処理）したのち、現在のworktreeで`git rebase <branch>`を実行します。

## マルチリポワークスペース

リポジトリを登録（`~/.box/repos/`にbare clone）し、セッション作成時に名前で指定します：

```bash
box repo add ~/projects/frontend
box repo add ~/projects/backend

box new my-feature --repo frontend --repo backend
```

セッション作成時にリポジトリは自動的にfetchされます。各リポジトリは`~/.box/workspaces/<session>/<repo>/`にセットアップされます。単一リポジトリの場合、ワークスペースパスはリポジトリのサブディレクトリに直接解決されます。

### プリセット

プリセットは、繰り返し使うリポジトリの組み合わせに名前を付けたものです：

```bash
box preset add work --repo frontend --repo backend   # 定義
box preset add work                                  # 対話的セレクター
box preset edit work                                 # リポジトリを更新（TUI）
box preset list                                      # 一覧
box preset remove work                               # 削除

box new my-feature --preset work                     # 利用
```

## オプション

### `box new`

| オプション | 説明 |
|--------|-------------|
| `<name>` | セッション名（必須） |
| `--repo <name>` | 含めるリポジトリ（複数指定可、`--preset`と排他） |
| `--preset <name>` | プリセットを使用（`--repo`と排他） |
| `--strategy <strategy>` | `worktree`（デフォルト）または`clone` |

### `box edit`

| オプション | 説明 |
|--------|-------------|
| `<name>` | セッション名（必須） |
| `--add <repo>` | リポジトリを追加（複数指定可、指定時はTUIをスキップ） |
| `--remove <repo>` | リポジトリを削除（複数指定可、指定時はTUIをスキップ） |

### `box list`

| オプション | 説明 |
|--------|-------------|
| `--project`, `-p` | 現在のプロジェクトのセッションのみ表示 |
| `--quiet`, `-q` | セッション名のみ出力 |

### `box remove`

| オプション | 説明 |
|--------|-------------|
| `<name>` | セッション名（省略時は対話的セレクターを起動） |
| `--all`, `-a` | すべてのセッションを削除（`<name>`と排他） |

## 環境変数

| 変数 | 説明 |
|----------|-------------|
| `BOX_STRATEGY` | デフォルトのワークスペース戦略（`worktree`または`clone`）。`--strategy`で上書き可能 |
| `BOX_VERBOSE` | 設定すると`--verbose`と同等 |
| `BOX_ROOT` | boxのデータディレクトリを上書き（デフォルト`~/.box`）。シェル補完が参照 |
| `BOX_POST_SWITCH_HOOK` | `box switch` / `box new`後に実行されるシェルスニペット。セッション名は`$BOX_SESSION_NAME`で参照可能。[シェル連携](#シェル連携)参照 |

## シェル連携

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

以下の機能が提供されます：

- セッション・リポジトリ・プリセットのタブ補完
- `box switch` / `box new`で作業ディレクトリを切り替える`box`シェル関数
- セッションへ入った後に実行される`BOX_POST_SWITCH_HOOK`（セッション名は`$BOX_SESSION_NAME`で参照可能）

### ポストスイッチフック

セッション切り替え・作成時に実行したいシェルスニペットを`BOX_POST_SWITCH_HOOK`に設定します（`box new`と`box switch`の両方で発火）。よく使う例：

```bash
# tmux — 現在のウィンドウをリネーム
export BOX_POST_SWITCH_HOOK='tmux rename-window "$BOX_SESSION_NAME"'

# zellij — 現在のタブをリネーム
export BOX_POST_SWITCH_HOOK='zellij action rename-tab "$BOX_SESSION_NAME"'

# kitty — タブタイトルを設定
export BOX_POST_SWITCH_HOOK='kitty @ set-tab-title "$BOX_SESSION_NAME"'

# 汎用 OSC 2 — 端末のウィンドウ／タブタイトルを設定
export BOX_POST_SWITCH_HOOK='printf "\033]2;%s\007" "$BOX_SESSION_NAME"'
```

スニペットは`eval`で現在のシェル内で実行されるため、`$TMUX`や`$ZELLIJ`などの環境変数も分岐に使えます。

## 仕組み

Boxは2つのワークスペース戦略をサポートしています：

### Worktree（デフォルト）

```
~/.box/repos/              box new my-feature        ~/.box/workspaces/my-feature/
  frontend.git    ──── git worktree add ─────>      frontend/
  backend.git                                       backend/
```

`git worktree`はbare repoにリンクされた軽量な作業ツリーを作成します。オブジェクトストアを共有するため、作成は即座に完了しディスク使用量も最小限です。各worktreeは`box/<session>`という独自のブランチを持ち、`box remove`でworktreeを適切にクリーンアップします。

### Clone

```
~/.box/repos/              box new my-feature        ~/.box/workspaces/my-feature/
  frontend.git    ──── git clone --local ────>      frontend/
  backend.git                                       backend/
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
| Bareリポジトリ | `~/.box/repos/<name>.git/` |
| ワークスペースの場所 | `~/.box/workspaces/<session>/` |
| セッションメタデータ | `~/.box/sessions/<session>/` |
| プリセット | `~/.box/presets/<name>` |
| デフォルト戦略 | `git worktree`（`--strategy clone`で変更可能） |
| クリーンアップ | `box remove`でワークスペースとセッションデータを削除 |

## ライセンス

MIT
