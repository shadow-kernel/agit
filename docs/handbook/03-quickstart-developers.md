# 3. Quick start for developers

This is a complete first session for a human developer who knows git. Every command below was run in order in a fresh directory; outputs are real and trimmed. You need `agit` on `PATH` ([2. Installation](02-installation.md)) and, for the last section, `git`.

Throughout, `--json` is shown where the structure matters. Every command accepts it.

## Create a repository and commit

```sh
$ mkdir demo && cd demo
$ agit init
Initialized empty agit repository in /…/demo/.agit
$ agit config user.name "Dana Developer"
$ agit config user.email dana@example.com
$ agit config --json
{"ok":true,"result":{"format_version":1,"default_branch":"main","user":{"name":"Dana Developer","email":"dana@example.com"},"remotes":{}}}
```

`agit root` works even before the first commit and tells you where everything is:

```sh
$ agit root --json
{"ok":true,"result":{"work_dir":"/…/demo","common_dir":"/…/demo/.agit","workspace":"main","branch":"main","head":null,"merge_in_progress":false}}
```

Add files and commit. `--intent` says why; `--task` links a ticket. Both are optional locally, but server policies can require them, so it is a good habit from the first commit.

```sh
$ printf '# demo\n\nA small project.\n' > README.md
$ mkdir src && printf 'fn main() {\n    println!("hello");\n}\n' > src/main.rs
$ agit status
On branch main

Untracked files:
	README.md
	src/main.rs
$ agit add README.md src
add      README.md
add      src/main.rs
$ agit status --json
{"ok":true,"result":{"workspace":"main","branch":"main","staged":[{"path":"README.md","kind":"added","new":{"id":"abda8e1f…","mode":"file","size":25}},{"path":"src/main.rs","kind":"added","new":{"id":"da3d0fe7…","mode":"file","size":37}}],"unstaged":[],"untracked":[],"conflicts":[],"merge_in_progress":false}}
$ agit commit -m "Initial commit" --intent "Bootstrap the demo project" --task DEMO-1
[main f61ab45c8fb6] Initial commit
 2 file(s) changed
```

A failed command is one JSON object and a non-zero exit code:

```sh
$ agit commit --json -m "x"; echo "exit=$?"
{"ok":false,"error":{"code":"nothing_to_commit","message":"nothing to commit","retryable":false,"details":null}}
exit=1
```

Look at history. Note the structured fields:

```sh
$ agit log
commit f61ab45c8fb68ad005dda8b4aff0c45edaa4677530af584d97c98853634954d1
Author: Dana Developer <dana@example.com>
Date:   2026-10-15T19:27:31Z
Intent: Bootstrap the demo project
Task:   DEMO-1

    Initial commit

$ agit log --json
{"ok":true,"result":[{"id":"f61ab45c…","tree":"4c96730f…","parents":[],"author":{"name":"Dana Developer","email":"dana@example.com"},"time_ms":1792092451181,"message":"Initial commit","intent":"Bootstrap the demo project","task":"DEMO-1"}]}
```

Objects behave as in git:

```sh
$ agit cat-file -t HEAD
commit
$ agit rev-parse HEAD --json
{"ok":true,"result":{"rev":"HEAD","id":"f61ab45c8fb68ad005dda8b4aff0c45edaa4677530af584d97c98853634954d1"}}
$ agit ls-tree HEAD -r
100644 abda8e1f… 25	README.md
100644 da3d0fe7… 37	src/main.rs
$ agit fsck --json
{"ok":true,"result":{"objects":5,"corrupt":[],"missing":[],"dangling_refs":[]}}
```

Tree listings show the file size without opening the blob; the size is stored in the tree entry.

## Branch, diverge, merge with a conflict

```sh
$ agit commit --json -m "Second" --intent "x" --allow-empty      # an empty commit to branch from
$ agit branch feature/greeting
Created branch feature/greeting at 707464cd6b1e
$ agit switch feature/greeting
Switched to branch 'feature/greeting'
$ printf 'fn main() {\n    println!("hello, world");\n}\n' > src/main.rs
$ agit commit -a -m "Greet the world" --intent "Make the greeting friendlier" --task DEMO-2
[feature/greeting 351a8c03e737] Greet the world
 1 file(s) changed
$ agit switch main
$ printf 'fn main() {\n    println!("hello, agit");\n}\n' > src/main.rs
$ agit commit -a -m "Greet agit" --intent "Mention the tool" --task DEMO-3
[main e1f412639dfe] Greet agit
 1 file(s) changed
$ agit branch --json
{"ok":true,"result":[{"name":"feature/greeting","id":"351a8c03…","current":false},{"name":"main","id":"e1f41263…","current":true}]}
```

`-a` stages modifications of tracked files, as in git. `switch` (alias `checkout`) refuses to overwrite local changes (`dirty_worktree`, exit 3) unless you pass `--force`; `switch -c name` creates the branch first; `switch <id>` detaches.

Now merge. Both branches changed the same line, so the merge stops with a conflict. With `--json` you get the full outcome (the conflicted paths, the blob ids of base, ours and theirs, and what was auto-merged), followed by the error envelope, and exit code 3:

```sh
$ agit merge feature/greeting --json; echo "exit=$?"
{"ok":true,"result":{"kind":"conflicts","tree":"84c98e13…","conflicts":[{"path":"src/main.rs","kind":"content","base":"da3d0fe7…","ours":"b5f319df…","theirs":"eb9b58f5…"}],"auto_merged":[]}}
{"ok":false,"error":{"code":"merge_conflicts","message":"merge produced conflicts in 1 file(s)","retryable":false,"details":{"count":1}}}
exit=3
```

(Yes, that is two JSON documents on one failure; see the [FAQ](15-faq.md#known-discrepancies-and-rough-edges). Read the first line for the details, the second for the code.)

The working tree now contains diff3-style markers, which include the base version:

```sh
$ agit status
On branch main
Merge in progress (fix conflicts and commit, or `agit merge --abort`)

Unmerged paths:
	both modified:   src/main.rs

Changes to be committed:
	modified:    src/main.rs
$ cat src/main.rs
fn main() {
<<<<<<< main
    println!("hello, agit");
||||||| original
    println!("hello");
=======
    println!("hello, world");
>>>>>>> feature/greeting
}
```

Resolve, stage, commit. The commit has two parents:

```sh
$ printf 'fn main() {\n    println!("hello, world and agit");\n}\n' > src/main.rs
$ agit add src/main.rs
update   src/main.rs
$ agit commit -m "Merge feature/greeting" --intent "Combine both greetings" --json
{"ok":true,"result":{"id":"a109bac6…","tree":"7f09e03b…","parents":["e1f41263…","351a8c03…"],"branch":"main","changes":[{"path":"src/main.rs","kind":"modified","old":{…},"new":{…}}],"summary":"Merge feature/greeting"}}
$ agit log --graph --oneline
* a109bac6bf39 Merge feature/greeting
|\
* | e1f412639dfe Greet agit
| * 351a8c03e737 Greet the world
| |/
* 707464cd6b1e Second
* f61ab45c8fb6 Initial commit
```

`agit merge --abort` restores the pre-merge state at any point before that commit. A fast-forward merge is reported as `{"kind":"fast_forward"}`, a real merge as `{"kind":"merged","commit":…,"auto_merged":[…]}`, nothing to do as `{"kind":"already_up_to_date"}`; `--no-ff` forces a merge commit.

Diffs between revisions, with stats:

```sh
$ agit diff --stat HEAD~2 HEAD
 src/main.rs | +1 -1
 1 file(s) changed, 1 insertion(s), 1 deletion(s)
$ agit diff --json HEAD~1 HEAD
{"ok":true,"result":{"changes":[{"path":"src/main.rs","kind":"modified",…}],"stats":[{"path":"src/main.rs","added":1,"removed":1}],"patch":"--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    println!(\"hello, agit\");\n+    println!(\"hello, world and agit\");\n }\n"}}
```

`agit diff` with no revision compares working tree and index; `--staged` compares index and HEAD; one revision compares it with the working tree. Renames are detected (`kind: "renamed"` with `from` and `similarity`).

## Stash

Stashes include untracked files:

```sh
$ printf 'pub fn helper() {}\n' > src/lib.rs
$ printf '# demo\n\nA small project. WIP\n' > README.md
$ agit stash --json
{"ok":true,"result":{"index":0,"id":"85b65fe0…","message":"WIP on main","time_ms":1792092601040,"base":"a109bac6…"}}
$ agit status --short                       # clean
$ agit stash list
stash@{0}: WIP on main (2026-10-15T19:30:01Z)
$ agit stash pop --json
{"ok":true,"result":{"kind":"committed","commit":"85b65fe0…","files_changed":0}}
$ agit status --short
 M README.md
?? src/lib.rs
$ agit add -A && agit commit -m "Add helper and README note" --intent "Prepare a helper module" --task DEMO-4
```

`stash push -m`, `apply`, `drop` and an index argument (`stash pop 1`) work as in git. A stash is applied as a three-way merge against its base, so it can conflict like any merge.

## Blame

```sh
$ agit blame src/main.rs
f61ab45c8fb6 (Dana Develo… 2026-10-15T19:27:31Z)    1: fn main() {
a109bac6bf39 (Dana Develo… 2026-10-15T19:27:48Z)    2:     println!("hello, world and agit");
f61ab45c8fb6 (Dana Develo… 2026-10-15T19:27:31Z)    3: }
$ agit blame src/main.rs --json
{"ok":true,"result":{"path":"src/main.rs","rev":"fa32a426…","lines":[{"line":1,"commit":"f61ab45c…","text":"fn main() {"},…],"commits":{"a109bac6…":{"id":"a109bac6…","author":"Dana Developer","time_ms":1792092468105,"summary":"Merge feature/greeting",…}},"truncated":false}}
```

Blame follows first-parent history of the commits that touched the path and stops after `--max-commits` (default 500); when it stops early, `truncated` is `true` and older lines are attributed to the oldest commit examined. When a commit has an `agent`, the agent id is shown instead of the author.

## Reflog and undo

Every move of HEAD and of every branch is logged. Reset moves the branch and is itself logged, so a mistaken reset is one more reset away from being undone:

```sh
$ agit reflog -n 3
fa32a4268082 HEAD@{0}: commit: Add helper and README note (danielbrueckmann, 2026-10-15T19:30:01Z)
a109bac6bf39 HEAD@{1}: commit: Merge feature/greeting (danielbrueckmann, 2026-10-15T19:27:48Z)
e1f412639dfe HEAD@{2}: commit: Greet agit (danielbrueckmann, 2026-10-15T19:27:48Z)
$ agit reset --hard HEAD~1 --json
{"ok":true,"result":{"from":"fa32a426…","to":"a109bac6…","mode":"hard","files_changed":2}}
$ agit reflog -n 2
a109bac6bf39 HEAD@{0}: reset: moving to HEAD~1 (danielbrueckmann, 2026-10-15T19:30:01Z)
fa32a4268082 HEAD@{1}: commit: Add helper and README note (danielbrueckmann, 2026-10-15T19:30:01Z)
$ agit reset --hard fa32a426808261a81cdefc55d7367c5e40a3177aed30b8a6b0973fec1e6914f6
HEAD is now at fa32a4268082 (Hard)
```

Two things to know: the `HEAD@{n}` labels are display only (`agit reset --hard 'HEAD@{1}'` answers `unknown_revision`), so copy the id; and the actor column is `$AGIT_AGENT`, else `$AGIT_AUTHOR_NAME`, else the OS user, not `user.name`. `agit reflog main --json` gives the entries of a branch as objects with `old`, `new`, `actor`, `time_ms`, `message`. `reset` without a mode flag and with paths unstages them; `--soft` moves only the branch, `--mixed` (default for a revision) also resets the index, `--hard` also the working tree.

## Checkpoints

A checkpoint snapshots the whole working tree, including untracked files, without touching HEAD or the index. Take one before anything risky:

```sh
$ printf 'pub fn helper() { todo!() }\n' > src/lib.rs
$ echo scratch > notes.txt
$ agit checkpoint -m "before rewriting helper" --json
{"ok":true,"result":{"id":"4cc2e92e…","tree":"b37c8a4e…","previous":null,"changes_vs_head":2,"created":true}}
$ agit checkpoint --if-changed --json                   # nothing changed since: created=false
$ rm notes.txt; printf 'broken\n' > src/lib.rs           # the risky thing went wrong
$ agit checkpoint restore 4cc2e92e… --json
{"ok":true,"result":[{"path":"notes.txt","kind":"added",…},{"path":"src/lib.rs","kind":"modified",…}]}
$ cat notes.txt
scratch
```

`agit checkpoint list` walks the chain newest first; `agit restore <rev>` accepts any commit or checkpoint and rewrites the working tree to it (`agit restore HEAD` discards all local changes). Checkpoints never show up in `agit log`; `agit gc` expires them after 30 days by default.

## Revert, cherry-pick, grep, tag

```sh
$ agit revert HEAD --json
{"ok":true,"result":{"kind":"committed","commit":"c2cc1204…","files_changed":2}}
$ agit cherry-pick HEAD~1 --json
{"ok":true,"result":{"kind":"committed","commit":"b906dfda…","files_changed":2}}
$ agit log --oneline -n 3
b906dfda4851 Add helper and README note
c2cc120464d4 Revert "Add helper and README note"
fa32a4268082 Add helper and README note
$ agit grep 'println' --json
{"ok":true,"result":[{"path":"src/main.rs","line":2,"text":"    println!(\"hello, world and agit\");"}]}
$ agit grep 'helper' --rev HEAD~2
src/lib.rs:1:pub fn helper() {}
$ agit tag v0.1.0 --json
{"ok":true,"result":{"name":"v0.1.0","id":"b906dfda…"}}
```

`grep` exits 1 when nothing matches (like git). Both `revert` and `cherry-pick` can stop with conflicts, reported like a merge.

## Garbage collection

`gc` removes unreachable objects older than a grace period (default 120 s), expires old reflog entries (90 d) and checkpoints (30 d), and repacks legacy loose objects:

```sh
$ agit gc --dry-run --json
{"ok":true,"result":{"objects_total":37,"objects_reachable":31,"objects_removed":3,"bytes_removed":421,"objects_kept_young":3,"reflog_entries_expired":0,"checkpoints_expired":0,"dry_run":true}}
$ agit gc --json
{"ok":true,"result":{…,"dry_run":false}}
$ agit fsck
34 object(s) checked
ok
```

Objects referenced from the reflog are kept until the entries expire, so `gc` never removes what `reflog` can still reach.

## Workspaces, leases and events in one minute

These are the multi-agent primitives; the full treatment is in [4](04-quickstart-agents.md) and [12](12-multi-agent-workflows.md). As a developer you will meet them when an agent shares your repository.

```sh
$ agit ws new coder-2 --agent coder-2 --json
{"ok":true,"result":{"name":"coder-2","path":"/…/demo.coder-2","created_ms":1792092645238,"agent":"coder-2"}}
$ agit ws list
* main
    /…/demo
  coder-2 (coder-2)
    /…/demo.coder-2
$ cat ../demo.coder-2/.agit                 # a pointer file, not a directory
{"common_dir": "/…/demo/.agit", "workspace": "coder-2"}
$ agit lease claim src/main.rs --holder coder-1 --ttl 30m --note "tidy main"
lease c618a7ee7a6c granted to coder-1 for src/main.rs
$ agit commit -a -m "Punctuation" --intent "Add exclamation mark" --agent coder-2 --json; echo "exit=$?"
{"ok":false,"error":{"code":"lease_conflict","message":"lease conflict: src/main.rs is held by coder-1 (lease c618a7ee7a6c)","retryable":false,"details":{"path":"src/main.rs","holder":"coder-1","lease_id":"c618a7ee7a6c"}}}
exit=3
$ agit lease release --all --holder coder-1
$ agit events --since 0 | tail -n 3
#28 2026-10-15T19:30:45Z lease.renew      main       coder-1 {"id":"c618a7ee7a6c","expires_ms":1792099845324}
#29 2026-10-15T19:30:45Z lease.release    main       coder-1 {"id":"c618a7ee7a6c","paths":["src/main.rs"]}
#30 2026-10-15T19:30:45Z lease.release    main       coder-2 {"id":"8397196cea77","paths":["docs/**"]}
$ agit ws remove coder-2 --delete
```

## Submodules

A submodule is a tree entry of mode `submodule` whose id is a commit of another repository; `.agitmodules` (TOML) maps the path to the URL. Local directories work as URLs:

```sh
$ agit submodule add ../lib vendor/lib --json
{"ok":true,"result":{"path":"vendor/lib","url":"../lib"}}
$ cat .agitmodules
[submodule."vendor/lib"]
path = "vendor/lib"
url = "../lib"
$ agit status --short
A  .agitmodules
A  vendor/lib
$ agit commit -m "Vendor lib" --intent "Pin the shared library"
$ agit submodule status --json
{"ok":true,"result":[{"path":"vendor/lib","url":"../lib","pinned":"2f43cacd…","actual":"2f43cacd…","state":"ok"}]}
$ agit ls-tree HEAD
100644 2fb2f36a… 60	.agitmodules
100644 5e75dcd5… 29	README.md
040000 c4bd23c8… 2	src
040000 fca4af59… 1	vendor
```

`agit submodule update` clones missing submodules and checks out the pinned commits (`state` is `ok`, `modified`, `missing` or `unpinned`); `agit add vendor/lib` re-pins after you move the nested checkout; `agit submodule remove <path> [--delete]` unregisters it.

Two caveats in 0.1.0, both tracked in the [FAQ](15-faq.md#known-discrepancies-and-rough-edges): `agit switch` to another branch fails with an `io` error in a repository whose tree contains a submodule entry (creating a branch with `switch -c` at the current commit still works), and clones of such a repository report the submodule path as modified until the nested repository is cloned. Keep submodules out of repositories where agents switch branches until that is fixed.

## Import an existing git project

The bridge uses the `git` binary. The full history, branches, tags, modes, symlinks and submodule pins are imported; `Intent:`, `Task:`, `Agent:` and `Model:` trailers in git commit messages become structured fields, and the original SHA is kept in `meta["git.sha"]`:

```sh
$ agit git import ../gitproj --into imported --json
{"ok":true,"result":{"commits":2,"trees":2,"blobs":1,"branches":["main"],"tags":[],"skipped_commits":0}}
$ agit -C imported log --json | jq '.result[0] | {message, intent, task, meta}'
{
  "message": "Add a",
  "intent": "provide a",
  "task": "GP-1",
  "meta": { "git.sha": "eacef3602e3f3ec89633e5f8a3450db0f9bd1a46" }
}
$ agit -C imported fsck
5 object(s) checked
ok
```

Without `--into` the import goes into the current agit repository; re-running it is incremental (`skipped_commits` counts what was already there, via `.agit/git-map.json`). `--branch <name>` restricts branches, `--no-tags` skips tags, `--no-checkout` leaves the working tree alone.

Export the other way with `git fast-import`; repeated exports are incremental too:

```sh
$ agit -C imported git export gitproj-export --json
{"ok":true,"result":{"commits":2,"reused":0,"branches":["main"],"tags":[]}}
$ git -C imported/gitproj-export log --oneline
8e3dce0 Add a
fb0cd09 root
```

Note that a relative `--into` or export target is resolved against `-C <dir>` when you pass one.

## Where to go next

- Everything above as a reference, with every flag and JSON shape: [6. CLI reference](06-cli-reference.md).
- Pushing to a server, pull requests and reviews: [4. Quick start for agents](04-quickstart-agents.md) (the server workflow is the same for humans).
- What the objects and refs on disk look like: [5. Concepts](05-concepts.md).
