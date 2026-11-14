# 0024: In auto, a tool's login is a grant for one call

Status: accepted, 2026-11-14.

## Context

The `auto` sandbox hides every secret: keys, token files, agent sockets, token
variables and the keyring. A command that logs in, such as `git push`, `gh pr create`
or `cargo publish`, therefore could not work in the sandbox. So efr ran each upload
outside the sandbox, with all of the user's rights, after the reviewer allowed it.

That design had three costs:

- An approved push also ran the code that the repository names, such as its hooks,
  outside the sandbox. efr checked that code before each outside git or gh line
  (`git_code`, about 1,400 lines of code and 1,500 lines of tests in four crates), and
  each review of that check found one more form.
- efr kept rules for single tools, such as which `gh api` forms write.
- The one rule that a tool's login is a resource like any other was missing. The user
  rejected two designs that were special to git and gh.

## Decision

1. efr splits its secrets in two. efr's own secrets (its keys in `secrets/`), the
   system's secrets and the secret entries of `/proc` stay floors: no grant opens them.
   Every other secret is a tool's login.
2. A tool's login is a grant for one call, in the sandbox: an `unmask` of a credential
   file or dir, a `socket` of an agent (the call keeps the variable that names it), an
   `env` variable that the sandbox removes as secret-like, or the `keyring` (the
   Secret Service on the call's filtered session bus). Only the user, or the reviewer
   when the user asked for the action, approves it.
3. efr does not know which login a tool uses. The model names it in `needs`, and the
   tool text tells it to do so on the first call.
4. Uploads run in the sandbox with those grants and the grants of their hosts. Only
   root and system commands (`sudo`, `docker`, systemd, persistence, a remote shell) and
   a line that the model asks to run outside run outside the sandbox.
5. A line that gets a login follows the one-command rule, as a line that runs outside.
6. efr keeps an upload verb only where a line can send data with nothing that efr
   grants: `git push`, curl and wget. A publish keeps its "the user named it" rule,
   because a version can never be published again. The other upload verbs (gh,
   `gem push`, `twine upload`, `scp`, `rsync`) and `gh_api_writes` go: their login is
   the exit.
7. `git_code` goes. A line that the model asks to run outside gets the strict review
   and a note that it runs the code that it starts.

## Consequences

- No per-tool table says where a login lives. The list of credential paths stays, to
  hide them from reads.
- A login reaches every program of its call, such as a hook or the build of a publish.
  The call stays in the sandbox, with no other secret and no write outside its roots,
  which is narrower than a run outside with all of the user's rights.
- The proxy carries no ssh, so a call to an ssh host gets the host's network.
- The keyring grant opens every unlocked entry of the Secret Service for its call; the
  bus filter cannot limit it to one entry.
- A file tool never reads a login.

## Alternatives

- A credential broker in efrd that holds tokens and answers git and gh: rejected by
  the user, because it puts tokens and new parsers into efrd.
- A design per tool (a git broker, a gh broker): rejected by the user; git and gh are
  tools like any other.
- Keep uploads outside the sandbox with `git_code`: the check has no end, and it costs
  more than the risk that it covers.
