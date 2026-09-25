---
author: Claude (Anthropic)
ai-generated: true
---

> **Provenance:** This file, and every other file in this directory, was
> written by Claude (Anthropic) while doing the work it describes. Reviewed in
> the normal course of review, but not audited line by line — treat specific
> numbers, paths, and version pins as claims to verify rather than guarantees.

# Implementation notes

Design notes, invariants, and build/release procedure for each surface of the
workspace. These are the *why* documents; the standing rules distilled out of
them — the ones that should be in mind before any change — live in
[AGENTS.md](../AGENTS.md) at the repo root.

| Document | Read it before you… |
|---|---|
| [core.md](core.md) | touch the matcher hot loop, add pattern syntax, change a `Limits` default, or quote a benchmark number |
| [gui.md](gui.md) | change Tauri command threading, add a window or menu item, or regenerate the desktop icon |
| [web.md](web.md) | change an `/api` route, a server-side limit, the Dockerfile, or anything in `deploy/` |
| [mobile.md](mobile.md) | build for a phone, touch `gen/`, or go near release signing and the mobile workflows |
| [versioning.md](versioning.md) | bump the version, or wonder where a version string comes from |

## Reference

[patterns.md](patterns.md) is different from the notes above: it is written for
*users*, as the complete rules of the pattern language. The maintainer's
[README](../README.md) and the in-app help give the overview; this is the fine
print. Every example in it was run against the committed `words.txt`, so a change
that moves one of its results is a change to the language, and the reference
must be updated in the same commit.

## About authorship

This repository distinguishes between documentation the maintainer wrote and
documentation Claude wrote, because some readers care about the difference and
should not have to guess.

- **Everything in this directory is AI-written.** Each file carries
  `ai-generated: true` in its YAML frontmatter and a prose provenance banner at
  the top. [AGENTS.md](../AGENTS.md) carries the same banner (without the
  frontmatter, which would otherwise be loaded verbatim into an agent's
  context).
- **[README.md](../README.md) and the in-app help source,
  [cha-gui/help/pattern-syntax.md](../cha-gui/help/pattern-syntax.md), are the
  maintainer's.** They are where the maintainer speaks to users in their own
  voice, so agents edit them only when asked. They carry no marker, and an
  unmarked document in this repository should be read as human-authored.
- **Commits already say so too.** This convention extends the existing
  `Co-Authored-By: Claude …` trailers to the file level, rather than
  introducing a new claim.

The banner's hedge is deliberate and should not be quietly upgraded. These
documents were written alongside the work, and the maintainer reviewed them in
the normal way — but "reviewed" is not "independently verified every figure",
and at this length the distinction matters. Where a document cites a
measurement, a file path, or a version pin, it is reporting what was observed
at the time of writing.

If you are a human contributor and you substantially rewrite one of these
files, amend its marker to say so, or drop the marker entirely — a stale
authorship claim is worse than none.
