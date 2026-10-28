# 0018. Account groups and the hand-off guard

- Status: Accepted · 2026-10-18 · the open item on limit-related claims resolved for display only by [0043](0043-plan-usage-display.md)
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0011](0011-keybindings.md), [0014](0014-v1-scope.md), [0016](0016-do-not-change-claude-code-defaults.md)

## Context

The hand-off ([0004](0004-account-hand-off.md)) moves a whole session to another
account. A review of Anthropic's public terms and docs, as of 2026-10-18, found
these points. This is a reading of public texts, not legal advice.

- **Terms follow the account.** Claude Code is governed by the Commercial Terms
  "for Team, Enterprise, and Claude API users" and the Consumer Terms "for Free,
  Pro, and Max users"
  ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)).
  Training and retention differ: for consumer accounts "We will train new models
  using data from Free, Pro, and Max accounts when this setting is on", while
  "Anthropic does not train generative models using code or prompts sent to
  Claude Code under commercial terms"
  ([data usage](https://code.claude.com/docs/en/data-usage)). Organisations also
  control logins through managed settings such as `forceLoginOrgUUID`
  ([authentication](https://code.claude.com/docs/en/authentication)). A hand-off
  from an employer's account to a personal one moves the employer's code and
  conversation under other terms.
- **Credentials stay with Claude Code.** Third-party "developers may not collect, store, or
  intermediate Claude.ai credentials or session tokens — sign-in to a Claude
  account must complete through Anthropic's own flow", and Anthropic does not
  permit them "to route requests through Free, Pro, or Max plan credentials on
  behalf of their users". Products that run Claude Code must leave the binary
  unmodified ("The Claude Code binary must not be modified.")
  ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)).
  The Consumer Terms add: "You may not share your Account login information
  […] or make your Account available to anyone else"
  ([consumer terms](https://www.anthropic.com/legal/consumer-terms)).
- **Limits are part of the service.** "Advertised usage limits for Pro and Max
  plans assume ordinary, individual usage of Claude Code and the Agent SDK"
  ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)).
  The Consumer Terms call such limits "technical restrictions"
  ([consumer terms](https://www.anthropic.com/legal/consumer-terms)). The Usage
  Policy effective November 12, 2026 prohibits to "Intentionally bypass or
  interfere with capabilities, restrictions, or guardrails established within
  our products", and to "create multiple accounts, to avoid detection,
  circumvent product guardrails" ([usage policy](https://www.anthropic.com/legal/aup)).
  The earlier version tied the first clause to "the purposes of instructing the
  model to produce harmful outputs"; the new one does not.
- **Local, single-user use is the supported shape.** "Agent view runs sessions
  on your machine and you dispatch each one"
  ([agent view](https://code.claude.com/docs/en/agent-view)).

Earlier docs described the hand-off as the way to continue when an account
hits its limits, and 0004 planned limit-based suggestions. Both read as a way
around limits.

## Decision

- **Groups.** Each account may set `group`, a string such as `"work"` or
  `"personal"`. Accounts without it share one implicit group.
- **The guard.** `a` offers only targets in the same group as the session's
  owner. The top-level boolean `allow_cross_group_handoff`, default `false`,
  lifts this. Across groups the hand-off is refused with a status message
  naming the groups, e.g. `hand-off from main (work) to alt (personal) is
  disabled: different account groups; set allow_cross_group_handoff = true to
  allow`. When the owner is unknown (`?`) and the accounts span several groups,
  it is refused until the owner is known (attaching once records it). The check runs when the picker opens and again right before the
  hand-off starts.
- **Always confirm.** Every hand-off asks, naming both accounts and their groups
  and stating that the full conversation and project context continue under
  the target account and its terms ([0011](0011-keybindings.md)). This replaces
  0004's rule of asking only for working or blocked sessions.
- **Manual only.** No automatic hand-off and no suggestion from usage or limit
  data, now or later ([0014](0014-v1-scope.md)). Docs describe the hand-off
  neutrally, as continuing a session under another of the user's own accounts,
  and point to Anthropic's official options for limits: usage credits,
  upgrading, API usage, or waiting for the reset.
- **Hard rules** restated in [0001](0001-thin-wrapper-over-claude-cli.md) and
  [0014](0014-v1-scope.md): no credential or token handling, no
  `claude setup-token`, no hosted, shared or multi-user mode, no proxying or
  routing of requests, a single-user local tool around the unmodified binary.
- The README carries an "Accounts and terms" section and a disclaimer that the
  project is independent and not affiliated with or endorsed by Anthropic.

## Consequences

- With the default config (no groups) nothing changes except the confirmation,
  which now appears for idle and stopped sessions too: one more key per
  hand-off.
- A user with employer and personal accounts can set groups and cannot move a
  session between them by mistake. Setting `allow_cross_group_handoff = true`
  is an explicit, visible choice in the user's own config.
- Groups do not separate data on disk. Config dirs that share history through
  symlinks still mix those accounts' transcripts and memory; the README says so.
- Tower cannot know which terms an account is under; the group is the user's
  own label.
- An unknown owner blocks hand-offs in a multi-group setup until it is learned.

## Alternatives considered

- **Confirm only.** It names the risk but still makes the cross-boundary move
  one key away; a config default is a stronger and clearer guard.
- **Docs only.** No code change, but nothing stops an accidental move between
  an employer's account and a personal one.
- **Remove hand-off.** The safest reading, but it drops a feature that is
  legitimate between a user's own accounts of the same kind, and Claude Code's
  `--resume` already allows it by hand.

## Open items

- Whether usage limits count as "restrictions" under the Usage Policy effective
  November 12, 2026. Until that is clear, tower stays manual and neutral and
  makes no limit-related claims. *Resolved for display only by [0043](0043-plan-usage-display.md):*
  the owner decided that tower may show, opt-in, the numbers Claude Code
  itself reports. Tower still makes no claims and no suggestions, and
  nothing of it reaches the hand-off (its picker, confirmation and status
  texts) or any automatic action.
- Cross-account messaging: ~~whether the orchestrator's `SendMessage` reaches
  sessions hosted by another account~~ it does: verified 2026-10-19 (Claude
  Code 2.1.303), and the reply comes back too ([0010](0010-orchestrator.md)).
  Still open: whether relaying across groups needs the same guard.
