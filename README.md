# revu

Review GitHub pull requests without leaving the terminal.

`revu` is a keyboard-driven TUI for the review loop: see what needs you
across all your repositories, read a PR like on its GitHub page, go through
the diff file by file, leave comments in a pending review, and jump around
the code (any file, go to definition, references) to check how a change fits.
It is built for large PRs, including AI-generated ones and the bot reviews
they come with.

- **Everything that involves you, across GitHub**: open PRs and issues
  grouped by repo, pinned repos first, then what waits on you (review
  requests, changes requested on your PRs, red CI, assignments). Switch to
  all the open items of your pinned repos, or of every repo you can access.
  No config needed.
- **PR page**: description, timeline (comments, reviews, threads), reviewers,
  assignees, labels, mergeability and checks.
- **Files changed**: syntax-highlighted diff with review threads inline,
  per-file "viewed" state synced with GitHub (applied instantly, synced in the
  background), lockfiles folded.
- **Reviews like on GitHub**: comment a line or a range, reply, resolve,
  then submit as comment / approve / request changes. Comments are written in
  a box inside revu, or in your `$EDITOR`.
- **Code navigation**: fuzzy-open any file of the PR head, `gd` / `gr` / `K`
  through a language server running on a detached worktree of the PR, jump
  list.
- **Local clones remembered**: run `revu` from a clone once, or clone from
  the list with `c`, and the repo gets local file reads and language servers.

Everything goes through the [GitHub CLI](https://cli.github.com/): if
`gh auth status` is fine, `revu` works, with whichever account `gh` is using
(switch with `gh auth switch`).

## Install

With Nix:

```sh
nix run github:alexpill/revu
# or keep it around
nix profile install github:alexpill/revu
```

With Cargo:

```sh
cargo install --git https://github.com/alexpill/revu
```

Either way, `revu` needs `gh` (logged in with `gh auth login`) and `git` in
`PATH`. Language servers are optional and looked up in `PATH` too (see below).

The first `nix run` on a machine builds `revu` from source, which downloads a
Rust toolchain; a binary cache is on the [roadmap](ROADMAP.md).

## Usage

```sh
revu                    # what involves you; run from a clone, that repo is pinned
revu --theme Nord
revu --pr 42            # just that PR of the clone you are in; q quits
```

Run from a clone, `revu` pins its repo for that run and remembers where the
clone is (its main checkout, if you are in a linked worktree). For a fork,
the `upstream` remote wins over `origin`.

Options: `--config <FILE>` to use another config file, `--theme <NAME>` to
override the theme, `--pr <NUMBER>` to open one PR alone, `--help`, `--version`.

### With Claude Code

`contrib/claude-code` is a Claude Code plugin: `/revu` opens revu on the
current branch's PR (or `/revu 42`) in a split beside Claude (herdr or tmux;
elsewhere it copies the command to run). In the diff, `a` on a line, a `v`
range or a thread opens the comment box, and what you write is sent to Claude
(framed as the plugin's, since it quotes others' code and comments), with the file, lines, diff and thread attached.

```sh
claude --plugin-dir /path/to/revu/contrib/claude-code
```

Set the plugin's `command` option (in `/config`) if `revu` is not on your
`PATH`, e.g. `nix run github:alexpill/revu --`.

## Configuration

`$XDG_CONFIG_HOME/revu/config.toml` (usually `~/.config/revu/config.toml`):

```toml
theme = "Catppuccin Mocha"     # optional, any bat theme name
clone_dir = "~/code"           # optional: where `c` clones repos (default ~/code)

[[repos]]                      # pinned: listed first, see the "pinned repos" view
name = "owner/repo"
path = "~/code/repo"           # optional: local clone (else the remembered one)

[[repos]]
name = "owner/other-repo"

[lsp]                          # optional: file extension -> server command
ts = ["typescript-language-server", "--stdio"]
nix = ["nixd"]
```

Local clones `revu` learns about (run from a clone, or cloned with `c`) are
remembered in `$XDG_STATE_HOME/revu/links` (usually
`~/.local/state/revu/links`), a file `revu` owns; a `path` in the config
wins over it. A repo with a local clone enables:

- reading files from the clone (`git show`), with no API size limits;
- language servers: on the first `gd`, `revu` fetches `refs/pull/N/head` and
  creates a detached worktree under `~/.cache/revu/worktrees/`. Your own
  checkout is never touched.

Default language servers: `rust-analyzer`, `typescript-language-server`,
`pyright-langserver`, `gopls`, `nil`, `lua-language-server`, `clangd`, `zls`,
`ruby-lsp`.

The first request starts the server and waits for it to load the project
(progress shows in the status bar); the answer appears once it is ready. If
a server misbehaves, `REVU_LSP_LOG=/tmp/lsp.log revu …` records everything it
sends, and a crash shows the end of its error output.

## Keys

Press `?` anywhere for the keys of the current screen.

**Home**

| Key | Action |
|---|---|
| `1`-`3`, `H`/`L`, `Tab` | activity (PRs and issues), pull requests, issues |
| `v` | view: involving you, pinned repos, everything you can access |
| `r` `a` `m` `d` | filter: review requested, assigned, mine, drafts (same key again: all); `f` cycles |
| `/` | filter by text |
| `Enter` | open a PR or issue, fold or unfold a repo |
| `c` | clone the repo into `clone_dir` |
| `o` | open in browser |
| `Ctrl-r` | refresh |

**Pull request**

| Key | Action |
|---|---|
| `1` `2` `3`… `H`/`L` | conversation, files, open buffers |
| `Tab` | files: switch between list and diff |
| `[` `]` / `{` `}` | previous / next file, hunk |
| `Space` | toggle viewed and move to the next unviewed file |
| `j/k h/l w/b 0/$ gg/G Ctrl-d/u` | move |
| `v`, then `c` | comment on a line or a range (pending review) |
| `r` / `R` | on a thread: reply / resolve or unresolve |
| `D` | on a comment: delete it (a pending one, or one of yours), after confirming with `y` |
| `z` | unfold a generated file |
| `gd` `gr` `K` | definition, references, hover |
| `Ctrl-p` | open any file of the PR head |
| `/` `n` `N` | search in a buffer |
| `Ctrl-o` | jump back |
| `x` | close buffer |
| `e` | open the current position in `$EDITOR` |
| `S` | submit review: comment, approve, request changes, or discard |
| `C` | comment on the PR |
| `a` | line, range or thread: ask Claude Code (revu started by `/revu`) |

Comments are written in a box: `Ctrl-s` sends, `Esc` cancels, `Ctrl-o` moves
the text to `$VISUAL` / `$EDITOR` (quit with `:cq`, a non-zero exit, to cancel).

## How it works

```
src/
├── main.rs          terminal setup
├── config.rs        config.toml, current-directory fallback
├── gh/              GitHub through `gh api`
│   ├── mod.rs       GraphQL/REST queries (lists, PRs, files, issues)
│   ├── model.rs     response types
│   └── mutation.rs  writes, run in order on a dedicated thread
├── source.rs        file contents: local clone or API; worktrees
├── diff.rs          patches to numbered lines (LEFT/RIGHT anchors)
├── highlight.rs     syntax highlighting (syntect + bat's syntaxes and themes)
├── markdown.rs      small Markdown renderer that strips bot HTML
├── lsp.rs           minimal LSP client (JSON-RPC over stdio)
└── app/
    ├── mod.rs       event loop, screen stack, background jobs
    ├── main_screen.rs  home: activity, PRs and issues by repo
    ├── issue.rs     issue page
    ├── picker.rs    fuzzy picker (files, LSP results)
    ├── ui.rs        shared rendering helpers
    └── pr/          pull request page
        ├── conversation.rs  description, timeline, sidebar
        ├── files.rs         file list, diff and threads
        └── code.rs          read-only code buffers
```

Anything doing I/O (`gh`, `git`, language servers) runs on a background
thread and reports back through one message channel, so the UI never blocks.
Writes (viewed state, comments, reviews) are applied locally first and sent
in order by a single worker.

A new screen is a `Screen` variant in `app/mod.rs` with `on_key`, `on_msg`,
`render` and `HELP`.

## Development

```sh
nix develop      # or any Rust toolchain >= 1.88
cargo test
cargo clippy
```

What is planned and the known limitations are tracked in
[ROADMAP.md](ROADMAP.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
