# 0013. No personal information in the repository

- Status: Accepted · 2026-10-18
- Related: [0015](0015-testing-strategy.md), [README](README.md)

## Context

The repository is public (MIT license). Tower is developed against real
machines with real accounts, so command output, logs and notes naturally
contain personal and environment details: e-mail addresses on status lines,
home paths, hostnames, session ids, account names. Once pushed, such details
cannot be reliably removed from git history or forks.

## Decision

Nothing committed may identify a person, machine or account. This applies to
code, docs, example config, test fixtures, commit messages, issue text and
screenshots.

**Forbidden**

- Real names, usernames, e-mail addresses, company names.
- Hostnames, IP addresses, terminal or machine names.
- Real paths (home directories, workspace folders). Write `~`,
  `~/workspace`, `~/.claude-alt`.
- Real account names. Use the example accounts `main` (command `claude`,
  config dir `~/.claude`) and `alt` (command `claude-alt`, config dir
  `~/.claude-alt`).
- Real session ids, `sessionId`s, session names, PIDs and timestamps.
- Tokens, credentials, plan or billing details.

**Private notes** live outside version control. Working notes with environment
details are listed in `.gitignore` and are never linked from committed files.

**Fixtures** captured from real machines are sanitized before they are
committed ([0015](0015-testing-strategy.md)):

- `id` and `sessionId` are replaced by made-up values of the same shape, e.g.
  `1a2b3c4d` and `1a2b3c4d-0000-4000-8000-000000000000`, consistently within a
  file.
- `name` becomes a neutral name such as `fix-flaky-test`; `pid` a value such
  as `4242`.
- `cwd` becomes a placeholder under a fixed fake home, e.g.
  `/home/user/workspace/demo-app`.
- Timestamps are shifted to a fixed base, keeping the differences between them.
- Raw `claude logs` streams are checked on the *rendered* screen as well as in
  the bytes, because cursor moves can split a string so that a byte-level grep
  misses it. Status lines (e-mail, plan, usage), prompts, paths and transcript
  text are replaced or the capture is redone in a throwaway project.

**Commit identity.** Commits use a no-reply address (e.g. the GitHub
`<id>+<user>@users.noreply.github.com` form), set per repository with
`git config user.email`.

**Screenshots and recordings** use a throwaway project with demo session names.

**Pre-commit check (suggested).** A local hook greps the staged diff before
each commit and blocks on a match:

```sh
git diff --cached -U0 | grep -nE \
  '[[:alnum:]._%+-]+@[[:alnum:].-]+\.[[:alpha:]]{2,}|/root/|/home/[a-z]|/Users/|[0-9]{1,3}(\.[0-9]{1,3}){3}' \
  | grep -vE 'noreply|/home/user/'
```

It can also read a git-ignored deny-list file with the contributor's own
personal words (username, hostnames, company), so those words never need to
appear in the repository themselves. The hook is a safety net, not a
substitute for reading the diff.

## Consequences

- Docs and fixtures read slightly artificially (`main`/`alt`, `demo-app`).
- Capturing a new fixture takes an extra sanitizing step and a review.
- Bug reports that include real output must be sanitized the same way before
  they are posted.

## Alternatives considered

- **Private repository.** Avoids the problem but rules out sharing the tool.
- **Sanitize automatically on capture only.** Useful as a helper, but byte
  streams and free-form text make a purely automatic pass unreliable; a human
  review stays required.
