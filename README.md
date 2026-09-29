# gitpanda

A fast git client in the spirit of GitKraken, written in Rust on
[gpui](https://crates.io/crates/gpui) (Zed's GPU UI framework), dressed in
Tokyo Night.

```sh
cargo run --release -- /path/to/repo     # or run inside a repo
```

## Features

- **Commit graph**: coloured lanes with curved merges, branch/tag/remote
  pills, a `// WIP` node for uncommitted work, virtualized so only visible
  rows are ever built.
- **Staging**: stage, unstage or discard whole files, **single hunks, or
  individual lines** (click lines, shift-click for a range, then use the
  hunk header buttons). Double-click a file to stage or unstage it.
- **Commit** with ⌘⏎, **amend** (preloads the last message).
- **Branches**: create, checkout (double-click), **rename**, delete, merge
  into current, rebase current onto, push / force-push with lease, pull,
  fetch. Remote branches check out as tracking branches.
- **Interactive rebase** editor: pick / reword / squash / fixup / drop,
  reorder, and edit messages. Shift-select commits → **Squash N commits**,
  or **Squash into parent** / **Reword** from any commit's menu.
- **Drop commits**: right-click → **Drop commit…**, or shift-select several
  → **Drop N commits…** (⌘⌫ works on the selection). The commits after them
  are rebased, and uncommitted changes are autostashed.
- **Conflict resolution**: a banner for merges, rebases, cherry-picks and
  reverts in progress (continue / skip / abort), and a resolver that shows
  ours and theirs per conflict: take ours, theirs, or both in either order,
  undo, then save and mark resolved.
- **Diffs** for working tree, index and commits, with **rename detection**.
  Files can also be renamed (`git mv`) from the file menu.
- Cherry-pick, revert, reset (soft / mixed / hard), tags, stashes (push,
  pop, apply, drop), commit details with parents and changed files.
- **Projects**: group related repositories into a project. The bar at
  the top has a tab per repository showing its branch, uncommitted changes,
  ahead/behind and any merge or rebase in progress, plus a project-wide
  tally. One repository is open at a time: click a tab or press ⌘1–⌘9 to
  switch (unsent commit messages are kept per repository). Opening a
  repository adds it to the active project. Projects are saved in
  `~/.config/gitpanda/projects`, or wherever `GITPANDA_PROJECTS` points.
- Live refresh: file-system watcher (ignores gitignored paths) plus refresh
  on window focus.

## Keys

| Key | Action |
| --- | --- |
| ↑ ↓ / j k | move through commits (shift extends the selection) |
| → / l | open the selected commit's first changed file |
| ↑ ↓ / j k (file open) | previous / next changed file |
| ← / h | close the file, back to the graph |
| ⌘⏎ | commit |
| ⌘B | new branch at selected commit |
| ⇧⌘S | stash |
| ⌘⌫ | drop selected commit(s) |
| ⌘R | refresh |
| ⌘O | open repository |
| ⌘1–⌘9 | switch to the project's nth repository |
| ⌘C | copy selected commit SHA |
| esc | close diff / dialog / menu |
| rebase editor | p r s f d set action, ⌥↑↓ reorder, ⏎ start |

Right-click commits, branches, tags, stashes and files for everything else.

## Speed

Reads use libgit2 on a background thread; the UI thread never touches
the repository. The walked history is cached and reused until a ref moves,
so the common reload (after an edit or a stage) only re-reads refs and
status. On ripgrep's history (2.3k commits, 289 refs) a cold load takes
~40 ms and a warm reload ~7.5 ms. Measure your own repository with:

```sh
cargo run --release -- --bench /path/to/repo
```

## Design

- `src/git/`: `repo.rs` builds snapshots (refs, history, status),
  `graph.rs` lays out lanes, `diff.rs` loads diffs and builds partial patches
  for hunk and line staging, `conflict.rs` parses and resolves conflict
  markers, `ops.rs` runs every mutation through the `git` CLI (so hooks,
  config and credentials behave exactly like your own git; interactive
  rebase is scripted through `GIT_SEQUENCE_EDITOR`).
- `src/ui/`: one root view (`app.rs`) owns all state; each region renders
  from its own module. Every list is a `uniform_list`.

## Tests

`cargo test` runs unit tests plus end-to-end tests against throwaway
repositories (line staging, hunk discard, squash, reword, fixup, merge
conflict resolution, branch rename, rename detection).

For UI work, `GITPANDA_SCRIPT` drives the live app and can screenshot it
(macOS; a process may always capture its own window), e.g.:

```sh
GITPANDA_SCRIPT="wait 1000; call select 3; shot /tmp/a.png; quit" cargo run -- .
```
