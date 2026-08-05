# Example workflows

Copy one, edit it, install it:

```bash
medulla workflow create --id pr-babysitter < examples/workflows/pr-babysitter.json
medulla workflow dry-run pr-babysitter --set pr=176
medulla workflow run     pr-babysitter --set pr=176 --set repo=owner/name
```

`dry-run` compiles the graph and walks it with agent and tool calls mocked, so
it proves the wiring and the `=` expressions without spending a harness session.
Every file here is checked by `feature_workflow_examples`, so an example that
stops parsing or validating fails the build.

| file | what it shows |
| --- | --- |
| [`review-a-repo.json`](./review-a-repo.json) | a single agent step over a repository |
| [`review-and-fix.json`](./review-and-fix.json) | a human approval gate between two steps |
| [`fan-out-per-file.json`](./fan-out-per-file.json) | `split_out` into a per-item fan-out |
| [`fix-until-tests-pass.json`](./fix-until-tests-pass.json) | a bounded `loop` around a fix attempt |
| [`pr-babysitter.json`](./pr-babysitter.json) | a loop with a `condition` branch, and a shell step that escapes into the operator's own login shell |
| [`ship-a-change.json`](./ship-a-change.json) | one workflow calling another through `sub_workflow` |

## One workflow calling another

`ship-a-change` opens a pull request and then hands it to `pr-babysitter`:

```json
{
  "id": "babysit",
  "kind": "sub_workflow",
  "config": {
    "workflow_id": "pr-babysitter",
    "inputs": { "pr": "=.nodes.locate.item.json.output.pr", "repo": "=inputs.repo" }
  }
}
```

`workflow_id` resolves against the workflows this host has installed — the same
ids `medulla workflow list` prints — so the child is versioned and edited on its
own, not copy-pasted into every caller. (The alternative, `config.workflow`,
embeds a child graph inline; provide exactly one of the two.)

`config.inputs` is resolved in the **parent's** scope and then validated against
the **child's** declared inputs, so a caller that forgets a required one fails
before the child executes anything. Types are checked without coercion: the
babysitter declares `pr` as a string, which is why the step above prints it as a
string rather than a number.

Nesting is bounded, not open-ended:

- A child that references its own `workflow_id` is refused before it runs.
- Every level increments a depth counter capped at 8 (raise or lower it with
  `max_sub_workflow_depth` on the **root** graph's trigger; the root's number is
  forwarded down, so the whole chain agrees on one bound). An indirect
  A → B → A cycle is caught by that counter rather than statically.
- The host's `workflows.maxLoopIterations` ceiling is re-applied to every
  resolved child, so a nested loop cannot outrun it by being one hop away.

Two things a sub-workflow cannot do today: pause on a `requires_approval` gate
(a child that pauses fails its parent rather than waiting for a human), and be
disabled — a disabled workflow is refused as somebody else's child too.

Set `execution: "per_item"` on the node to run one full child per input item,
with `concurrency` bounding how many run at once. That is how an array of work
becomes N parallel multi-step runs; the siblings share a depth, so a fan-out
widens a run without deepening it.

## Choosing the interpreter

`args.language` decides which interpreter a script runs under, and which file
extension it is staged with:

| `language` | program | invoked as |
| --- | --- | --- |
| `shell` (default) | `bash` | `bash <script> <input.json>` |
| `python` | `python3` | `python3 <script> <input.json>` |
| `javascript` | `node` | `node <script> <input.json>` |

For `shell`, the program is a default rather than a constant. It matters
because a script is run **non-login and non-interactive**: `~/.zshrc`,
`~/.bash_profile`, and anything they put on `PATH` are not in scope. What *is*
inherited is the daemon's own environment, plus whatever `args.env` adds. A
step that needs the operator's own shell — their functions, their `~/bin`,
their helper scripts — has to say so.

Per step, with `args.shell` and `args.shell_args`:

```json
{ "slug": "medulla:shell",
  "args": { "script": "pr-comments \"$PR\" --json", "shell": "zsh", "shell_args": ["-l"] } }
```

Or host-wide, in `<medulla home>/config.toml`:

```toml
[workflows]
shell = "user"        # follow $SHELL; or name one: "zsh", "/bin/zsh"
shellArgs = ["-l"]    # a login shell, so PATH and functions come with it
```

`args.shell` takes the same values `workflows.shell` does, `"user"` included —
a single step can follow the login shell without the host committing to it.

`shell = "user"` is the opt-in that makes scripts run under whatever the
operator actually uses. It is opt-in on purpose: an existing workflow's scripts
were written against `bash`, and re-running them under `fish` because that is
what `$SHELL` says would break them in ways that look like the script's fault.
Unset keeps `bash`.

Add `-i` alongside `-l` if the command you need is an *alias* — aliases are
never exported, so a login shell alone will not find one.

### Which shell the *body* is written in

Whatever you point a step at has to be able to run the script you wrote. A
POSIX body — `set -u`, `[ … ]`, `${VAR:-}` — is not valid fish, so
`shell = "user"` on a fish host does not give that step the operator's
environment, it fails the step outright. That is the trap: the option exists
for people with a non-default shell, and naively applied it breaks for exactly
them.

So when only *part* of a step needs the operator's environment, keep the body
portable and send just that part across. `pr-babysitter`'s inspect step is the
worked example — the script is POSIX and runs on the step's default shell, and
the one operator-supplied command goes through:

```bash
feedback=$("${SHELL:-/bin/sh}" -l -c "$FEEDBACK_COMMAND" 2>/dev/null || true)
```

Reach for `shell`/`shellArgs` on the step itself when the *whole* body is
written for that shell, and for the inner-invocation form when it is not.

Three things to keep in mind. `medulla:shell` needs `workflows.allowCode`,
which is on by default and runs with the daemon's full privileges. A script
that exits non-zero fails the node, so a command whose *failure* is the signal
you want (`gh pr checks` exits non-zero when checks are red) needs `|| true`
and a check on its output instead. And `shell`/`shell_args` apply to
`language: "shell"` only — naming one alongside `python` is refused rather than
silently handing a `.py` file to a shell.
