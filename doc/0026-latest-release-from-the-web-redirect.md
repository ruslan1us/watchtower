# 0026. Latest release from the web redirect, the API as fallback

- Status: Accepted · 2026-10-19
- Related: [0012](0012-build-and-distribution.md), [0023](0023-self-update.md)

## Context

`tower update` and `tower update --check` found the latest version through the GitHub
REST API, `https://api.github.com/repos/ruslan1us/watchtower/releases/latest`
([0023](0023-self-update.md)). Unauthenticated, that API allows about 60 requests an
hour per IP address. Behind a shared egress address (a company network, a NAT, a CI
runner) other users spend that budget, and the lookup failed with
`curl: (22) The requested URL returned error: 403`. `tower update --version vX.Y.Z`
worked because it skips the lookup.

The web page `https://github.com/ruslan1us/watchtower/releases/latest` is not subject to
that limit. GitHub answers it with `302 Found` and a `Location` of
`https://github.com/ruslan1us/watchtower/releases/tag/<tag>` for the newest normal (not
draft, not pre-release) release. A repository with no release redirects to
`https://github.com/ruslan1us/watchtower/releases` instead.

## Decision

This amends the "Which release" part of [0023](0023-self-update.md); everything else
there stands.

- **The web redirect first.** Tower runs
  `curl -q -fsS --proto =https --tlsv1.2 --max-time 120 -o /dev/null -w %{redirect_url} https://github.com/ruslan1us/watchtower/releases/latest`
  directly, with no shell. There is no `-L`: the redirect itself is the answer, `-w`
  prints its target, and `-o /dev/null` drops the short 302 body. Following the
  redirect (`-L -w %{url_effective}`) would download the whole release page for nothing,
  and a HEAD request is handled less uniformly by proxies than a GET.
- **Strict parsing.** The target must be exactly
  `https://github.com/ruslan1us/watchtower/releases/tag/` followed by a tag that parses
  as a version and reads back as the same `vX.Y.Z[-PRE]`. A redirect to `…/releases`
  means "no release found". No redirect, another host, scheme, owner or repository,
  or a malformed tag is an error.
- **The API as fallback.** When the web lookup fails for any reason, tower asks the API
  as before (`tag_name` from the JSON). When the API fails with HTTP 403 or 429, the
  message says GitHub's API rate limit for this network address is used up, and keeps
  curl's own text.
- **Both failed.** The error is one line naming both reasons, ending with the way around
  it: ``cannot look up the latest release: releases page: <why>; GitHub API: <why>;
  `tower update --version vX.Y.Z` skips the lookup``. Without `curl` on `PATH` only the
  existing "not installed" message is shown, once.
- **Unchanged.** `--check` uses the same lookup. The binary and its `.sha256` still come
  from the versioned `releases/download/vX.Y.Z/` URLs, so both belong to the release the
  lookup named.

## Consequences

- The lookup works on shared addresses that exhaust the API budget, and costs no API
  request while the web page answers. The API is called only when the page fails.
- Tower depends on GitHub's redirect format for `releases/latest`. If it changes, the
  strict parsing rejects it and the API fallback still answers.
- A failed lookup is still an error (exit 1 for `--check`), but the message says what to
  do instead.

## Alternatives considered

- **The API only, as in [0023](0023-self-update.md).** It fails behind busy shared
  addresses, which is how this decision came about.
- **An API token.** It raises the limit, but tower would have to find, store or ask for a
  credential for a public, read-only lookup.
- **Writing to `releases/latest/download/…` directly** (as 0023 also rejected). It skips
  the lookup, but the version is known only after the download, and the binary and the
  checksum could come from two different releases if one is published in between. The
  redirect gives the tag first, so both files are fetched from the same versioned URLs.
- **Parsing the release page's HTML.** It downloads far more and depends on markup that
  changes without notice; the redirect carries the same answer in one header.
