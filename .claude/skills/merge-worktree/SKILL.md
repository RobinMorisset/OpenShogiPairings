---
name: merge-worktree
description: Land the current git worktree's branch onto the default branch and clean up — rebase onto main, fast-forward main, then remove the worktree directory and delete the branch. Use whenever the work in a worktree is finished and the user wants it merged, landed, or cleaned up: "rebase on main and fast-forward merge", "merge this worktree into main", "land this branch and delete the worktree", "exit the worktree and remove it", "I'm done with this worktree". Use it even when the user names only part of the sequence (just the merge, or just the cleanup) — the checks here are what keep the rest of the sequence from destroying work.
---

# Landing a worktree branch on main

Worktrees created by the **Claude Desktop app's worktree toggle** are ordinary
`git worktree` checkouts under `<repo>/.claude/worktrees/<name>`. Nothing
special manages them, so the whole lifecycle is plain git.

## Do not reach for ExitWorktree

`ExitWorktree` only touches worktrees that `EnterWorktree` created *in the same
session*. For an app-created worktree it is a **no-op**:

> No-op: there is no active EnterWorktree session to exit. This tool only
> operates on worktrees created by EnterWorktree in the current session.

Skip it and use git. Calling it first is harmless but wastes a turn, and the
error is easy to misread as "something is wrong" rather than "wrong tool".

## 1. Establish the terrain

Everything downstream needs the **primary** worktree path — git always lists it
first — and the branch you are standing on.

```bash
git worktree list --porcelain | awk '/^worktree /{print $2; exit}'
```

Then, in one look: your branch, the default branch's position, whether the
default branch has moved since you branched, and whether the primary worktree
is clean.

```bash
PRIMARY=$(git worktree list --porcelain | awk '/^worktree /{print $2; exit}')
BRANCH=$(git rev-parse --abbrev-ref HEAD)
git status --short
git log --oneline -1 main
git merge-base --is-ancestor main HEAD && echo "ff possible" || echo "main moved — rebase needed"
git -C "$PRIMARY" status --short
```

Substitute `master` if that is the repo's default branch (`git symbolic-ref
--short refs/remotes/origin/HEAD` usually says which).

**Stop and ask the user** if `git status --short` shows modified tracked files.
Uncommitted work is the one thing this whole procedure can silently destroy, and
it is not yours to decide about. Untracked files in the *primary* worktree are
fine — they do not block a fast-forward.

## 2. Rebase

```bash
git rebase main
```

If it conflicts, stop and hand the conflict to the user with what you see. Do
not improvise a resolution: you are about to fast-forward main onto the result,
so a wrong call here lands directly on the trunk.

## 3. Re-verify — but only if main actually moved

This is the step that is easy to skip and occasionally expensive. When main has
moved, the rebased combination is code **no one has ever built or tested** — your
commit was verified against the old main, and the tests that passed before the
rebase prove nothing about after it. Run whatever the repo verifies with (its
pre-commit hook, or its CI equivalent — for a Rust repo typically
`cargo clippy --workspace --all-targets` and `cargo test --workspace`).

If `git merge-base --is-ancestor main HEAD` already said "ff possible" in step 1,
the rebase was a no-op and there is nothing new to verify. Say so and move on
rather than burning a few minutes on a redundant test run.

## 4. Fast-forward main from the primary worktree

The default branch is checked out in the primary worktree, and git will not
check out one branch in two worktrees. So the merge runs *there*, via `-C` —
never by cd'ing.

```bash
git -C "$PRIMARY" merge --ff-only "$BRANCH"
```

`--ff-only` is deliberate: after a successful rebase this must be a
fast-forward, so if git refuses, something changed under you (main moved again
during the run). Rebase again rather than reaching for a merge commit.

## 5. Stop anything still running out of the worktree

Dev servers, watchers and background jobs started during the session have the
worktree as their working directory, and a server built there is executing a
binary inside its `target/`. Deleting the directory does not stop them: a
process survives its own deleted binary quite happily, so it can go on holding
its port serving code that no longer exists on disk — the next session's server
then silently lands on a different port, or worse, answers from the build you
just deleted.

Stop them **before** the removal, not after. `preview_list` names what this
session started; `preview_stop` each `serverId`.

Then confirm for yourself, because the tool's word and reality disagree in
exactly the case that matters: once the session has been rebound (or the
harness has dropped its handles), `preview_list` comes back empty while the OS
process is still up. An empty list is not evidence.

**Identify by working directory, never by port.** The ports come from the
repo's `.claude/launch.json`, so they belong to the *project*, not to this
worktree — a server on 5173 may well be another worktree's session, doing
useful work for someone else. Killing whatever holds the port is how you take
down a colleague's dev server, or your own in the next tab. This is also the
second reason to do this before the removal rather than after: while the
directory still exists, a process's cwd is a path you can compare.

```bash
WT=$(pwd)                                   # the worktree about to be removed
pgrep -fl "$WT"                             # anything running from inside it
for port in <ports from .claude/launch.json>; do
  for pid in $(lsof -nP -tiTCP:"$port" -sTCP:LISTEN); do
    printf '%s\t%s\t%s\n' "$port" "$pid" \
      "$(lsof -a -p "$pid" -d cwd -Fn | sed -n 's/^n//p')"
  done
done
```

Kill only the pids whose cwd is under `$WT`. A port held by a process rooted
anywhere else is somebody's live server: leave it, and say in your report that
you found it and left it alone. Nothing rooted in the worktree is the thing to
see.

## 6. Prove there is nothing to lose, then delete

Three questions, all of which should come back empty:

```bash
git status --short                 # uncommitted work
git log --oneline main..HEAD       # commits not on main
git stash list                     # stashes (repo-wide, not per-worktree)
```

Then remove the worktree and the branch. **Combine removal and verification
into a single command**: the worktree directory is your own working directory,
and once it is gone, later shell invocations have no valid cwd to start in.

```bash
PRIMARY=/absolute/path/to/repo
WT=$PRIMARY/.claude/worktrees/<name>
git -C "$PRIMARY" worktree remove "$WT" && echo "worktree removed"
git -C "$PRIMARY" branch -d claude/<branch-name>
git -C "$PRIMARY" worktree list
git -C "$PRIMARY" log --oneline -2
test -e "$WT" && echo "STILL THERE" || echo "gone"
```

Two safety nets worth keeping rather than overriding:

- `git worktree remove` refuses a worktree holding modified tracked files or
  untracked ones — *exactly* the set `git status --short` lists, because git
  uses the same porcelain check. Ignored files do not count, so build artefacts
  (`target/`, `node_modules/`) never block removal; they are simply deleted with
  the directory, which is why removal can pause on a large build cache. The
  upshot: if step 6 came back empty, this cannot complain — and if it does
  complain, step 6 already showed you what. Look at that, do not add `--force`.
- `git branch -d` (lowercase) refuses to delete an unmerged branch, so git
  itself double-checks step 4. `-D` would throw the commits away silently.

## 7. Tell the user the session's cwd is gone

The directory the session was running in no longer exists. The harness normally
rebinds the session to the primary repo and says so; if it does not, further
commands will need a fresh session started in the primary repo. Either way, say
it plainly — it is surprising otherwise.

Close by reporting what landed where: main's new commit, and confirmation that
the worktree and branch are gone. Mention any *other* worktrees still present,
since `git worktree list` output makes it easy for the user to wonder whether
you touched them.
