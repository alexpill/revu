# Roadmap

What `revu` does not do yet, roughly in priority order. Known limitations of
the current version are listed at the end.

## Next

- [ ] **Check the remaining review writes against a real PR.** Replies,
      resolve/unresolve, range comments, submit (comment/approve/request
      changes), discard and PR comments are implemented against the GitHub
      GraphQL API but not exercised end to end yet. Viewed toggles, adding a
      line comment to the pending review and deleting it have been.
- [ ] **CI for this repo**: `cargo test`, `cargo clippy`, `nix build` on push.
- [ ] **Binary cache** (Cachix, pushed from CI and declared in the flake's
      `nixConfig`) so `nix run` downloads the binary instead of a Rust
      toolchain and a build.
- [ ] **Screenshots / demo GIF** in the README.
- [ ] **AI actions**: send the current hunk, file or whole PR to an AI CLI
      (e.g. `claude -p`) to explain it or pre-review it, and show the answer in
      a panel or as draft comments. Natural hook: a new `External` variant that
      runs a command with the terminal, or a background job feeding a popup.
- [ ] **Custom commands** in the config (key + shell command with placeholders
      such as `{repo}`, `{pr}`, `{path}`, `{line}`): the first step towards
      plugins, and enough to wire AI tools without hard-coding one.

## Review experience

- [ ] Side-by-side diff mode.
- [ ] Horizontal scrolling (long lines are cut today).
- [ ] Expand context around hunks (the full file is already available).
- [ ] Directory tree in the file list, with folding.
- [ ] Diff for files GitHub returns no patch for (binaries aside): compute it
      from the local clone.
- [ ] **Vim mode in the comment box.** It is a `ratatui-textarea` with its
      Emacs-like keys today; add Normal / Insert / Visual modes on top
      (`hjkl`, `w`/`b`/`e`, `0`/`$`, `x`, `dd`, `o`/`O`, `u`/`Ctrl-r`, `y`/`p`,
      counts), with the mode shown in the box border, like the `vim` example
      of ratatui-textarea. Optional (`[compose] vim = true`?) so the box stays
      usable for non-Vim users; `Ctrl-o` to `$EDITOR` remains for the rest.
      Do it with a shared `app/vim.rs` that turns keys into motions (count,
      pending `g`/`f`/`t`), used by the diff, the code buffers and the box
      (motions mapped to `CursorMove`), instead of the per-view key matches
      in `files.rs` and `code.rs` today; the motions of the "Read-only
      Vim-like editor" section then land everywhere at once.
- [ ] Edit your own comments; suggestion blocks; reactions.
- [ ] Delete PR-level (issue) comments; review comments can be deleted already.
- [ ] Filter threads by author (e.g. hide or show only bot reviews), jump
      between unresolved threads.
- [ ] **Optimistic comments, like the viewed toggle.** Adding, replying to,
      deleting and resolving comments should update the screen at once
      (a new comment shows as "sending", a deleted one disappears) while the
      GitHub write runs in the background, instead of waiting for the PR to
      reload. Show the sync state: a spinner or counter of writes in flight,
      per-comment status (sending / failed), and on failure restore the
      previous state with the error and a way to retry. Today these writes
      wait for GitHub, then re-fetch the whole PR.

## Pull request actions

- [ ] Merge, mark ready for review / convert to draft, request reviewers,
      edit labels and assignees.
- [ ] CI: list failing jobs, open logs, re-run.
- [ ] Check out the PR locally (`gh pr checkout`).

## Code navigation

- [ ] Diagnostics from the language server on changed lines.
- [ ] Clean shutdown of language servers; command to remove old worktrees
      (`~/.cache/revu/worktrees` is never cleaned today).
- [ ] **Decide on symbol navigation** and its keys: a picker of the symbols
      of the current file (LSP `textDocument/documentSymbol`, like VS Code's
      Ctrl+Shift+O or Telescope's document symbols) and of the whole repo
      (`workspace/symbol`, like Ctrl+T). `Ctrl-o` is already "jump back" as in
      Vim, so pick keys that don't clash (e.g. `Ctrl-t`, `gs`/`gS`), or decide
      it's not worth it.
- [ ] **Language servers for every language**, not just the built-in table:
      a much larger default list (e.g. derived from nvim-lspconfig or Helix's
      `languages.toml`), language ids for any extension, several servers per
      language, and project-root detection for monorepos (start the server at
      the nearest `package.json`/`tsconfig.json`/`Cargo.toml`/`go.mod`/…
      instead of the repo root).

### Read-only Vim-like editor

Make buffers and the diff feel like Neovim, still without editing:

- [ ] More motions: `e`/`ge`, `f`/`t`/`;`/`,`, `%`, `{`/`}` paragraphs in
      buffers, `H`/`M`/`L`, `zz`/`zt`/`zb`, counts (`5j`), marks and `''`.
- [ ] `*` / `#` and `/` search in the diff too, with match highlighting.
- [ ] Visual mode with yank to the system clipboard (`y`), line numbers
      and paths (`yp` for `path:line`).
- [ ] Splits (diff + definition side by side) and window motions.
- [ ] Folding, horizontal scrolling, soft wrap toggle.
- [ ] `:` command line (`:e path`, `:123`, `:q`) and configurable key maps.

## Dashboard and issues

- [ ] **Review progress in the PR lists** (home tabs): files viewed
      out of the total (e.g. `12/40`, or a small bar), so you can tell at a
      glance what is left to review, plus your pending comments if any. The
      list query would need each PR's files and `viewerViewedState`, which
      makes an already slow GitHub query heavier: probably fetch it lazily
      (visible rows only, or a second request after the list shows).

- [ ] More than 50 PRs and 50 issues per view scope (pagination / load more).
- [ ] Review requests made to a team you belong to.
- [ ] Auto-refresh and notifications.
- [ ] Create issues; open any PR or issue by reference
      (e.g. `revu owner/repo#123`).
- [ ] Search closed PRs and issues with GitHub syntax (`is:closed label:bug`),
      which the per-repo issues tab did before the home became cross-repo.

## Platform

- [ ] **Command palette** (`Ctrl-k`, or `Ctrl-p`/`:` if those fit better):
      one fuzzy prompt over the whole context (repos, PRs, issues, files of
      the open PR, comments) and over actions (approve, submit review, open
      in browser, check out, custom commands…), each action showing its key
      so the palette also teaches the shortcuts. Builds on the `/` search.
- [ ] Configurable key bindings.
- [ ] Remember UI state (last tab, filters) between runs.
- [ ] Plugin system beyond custom commands.
- [ ] Publish on crates.io and nixpkgs.

## Known limitations

- Each view loads at most the 50 most recently updated open PRs and 50
  issues per search scope (involving you, your account and orgs, pinned
  repos).
- "Review requested" only counts requests made to you directly.
- `e` opens the local clone when no worktree exists yet; it may not be on the
  PR branch.
- Wide characters (emoji, CJK) can shift the cursor cell by one column.
