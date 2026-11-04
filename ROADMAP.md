# Roadmap

What `revu` does not do yet, roughly in priority order. Known limitations of
the current version are listed at the end.

## Next

- [ ] **Check review writes against a real PR.** Line/range comments, replies,
      resolve/unresolve, submit (comment/approve/request changes), discard and
      PR comments are implemented against the GitHub GraphQL API but have not
      been exercised end to end yet. The viewed toggle has been.
- [ ] **CI for this repo**: `cargo test`, `cargo clippy`, `nix build` on push.
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
- [ ] Edit and delete your own comments; suggestion blocks; reactions.
- [ ] Filter threads by author (e.g. hide or show only bot reviews), jump
      between unresolved threads.
- [ ] Show the pending comment right away instead of after the reload.

## Pull request actions

- [ ] Merge, mark ready for review / convert to draft, request reviewers,
      edit labels and assignees.
- [ ] CI: list failing jobs, open logs, re-run.
- [ ] Check out the PR locally (`gh pr checkout`).

## Code navigation

- [ ] Language server progress (`$/progress`) in the status bar while indexing.
- [ ] Diagnostics from the language server on changed lines.
- [ ] Clean shutdown of language servers; command to remove old worktrees
      (`~/.cache/revu/worktrees` is never cleaned today).
- [ ] Workspace symbol search.

## Dashboard and issues

- [ ] More than 30 open PRs per repo (pagination / load more).
- [ ] Review requests made to a team you belong to.
- [ ] Auto-refresh and notifications.
- [ ] Create issues; open PRs and issues from outside the configured repos
      (e.g. `revu owner/repo#123`).

## Platform

- [ ] Configurable key bindings.
- [ ] Mouse support.
- [ ] Remember UI state (last tab, filters) between runs.
- [ ] Plugin system beyond custom commands.
- [ ] Publish on crates.io and nixpkgs.

## Known limitations

- Only the 30 most recently updated open PRs per repo are loaded (larger
  pages made GitHub's GraphQL API slow enough to return 502s).
- "Review requested" only counts requests made to you directly.
- `e` opens the local clone when no worktree exists yet; it may not be on the
  PR branch.
- Wide characters (emoji, CJK) can shift the cursor cell by one column.
