---
name: changelog-cleanup
description: Prune and condense the Unreleased section of CHANGELOG.md, which grows verbose because each session appends its own entry. Use when asked to clean up, trim, tidy or review the changelog, or before cutting a release. Covers which entries to delete outright (fixes to features that no release has yet shipped), how to check that against the release tags, how short an entry should be, and which entries to merge.
---

# Cleaning up the Unreleased changelog

Entries accumulate one session at a time, each written by whoever just finished
the work and still has every detail in mind. The result is accurate and far too
long: rationale, mechanism, and the shape of the bug before it was fixed. This
pass turns that back into something a referee scans.

**The reader is a referee, not a reviewer.** Each line has to earn one of two
reactions: *"that sounds like a feature I should look at"* or *"that bug bit me,
it's fixed now"*. Anything that serves neither — why the bug existed, how the
fix works, which module changed — goes.

Only ever touch `## [Unreleased]`. Released sections are history; leave them
alone even when they are wordy.

## 1. Find what is new

```bash
git log --oneline -S'Cleanup changelog' -- CHANGELOG.md   # or the last cleanup commit
git diff <last-cleanup> HEAD -- CHANGELOG.md
```

Everything added since that commit is the work of this pass. Entries that
survived a previous cleanup were curated deliberately — re-read them, but don't
churn them for style. Merging one into a new neighbour is fine; rewriting it
because you'd have phrased it differently is not.

## 2. Delete fixes to features no release has shipped

This is the rule that removes the most, and the only one worth research. A bug
in a feature that is *itself* in this same Unreleased section never existed for
any user: fix and feature ship together, so only the feature is news.

Do not decide this from the entry's wording. Check:

```bash
git tag                                              # newest tag = last release
git log --oneline --reverse -S'<identifier>' -- crates/   # when the feature landed
git merge-base --is-ancestor <commit> <newest-tag> && echo shipped || echo unreleased
```

Pick an identifier the feature cannot exist without — a struct, a config field,
a route (`is_published`, `OSP_ADMIN_PASSWORD`). The entry's own vocabulary is
unreliable: a fix to a *released* subsystem is often described through the new
feature that exposed it, and reads as new when it isn't.

Two failure modes, in both directions:

- **A commit can fix both.** One that repairs player removal in general *and* in
  team tournaments contributes only its released half to the changelog. Split
  it; don't drop the entry because part of it is new.
- **A new feature can expose an old hole.** A tournament with no password had no
  auth gate as far back as the last release; publishing merely made its id
  discoverable. That is a released security bug, and it stays — described
  without the new feature's mechanics.

Simulator-only and internal-tooling fixes go too, unless a referee could have
hit them.

## 3. Cut each survivor to its claim and its consequence

Target: **one to four lines.** A bolded lead clause naming the symptom, then
what it means for the reader. State the fix only when the new behaviour isn't
obvious from the symptom.

Drop: the mechanism ("the bracket is rebuilt from the frozen seeding on every
read"), the enumeration of cases now validated, file and function names,
reassurances that nothing else changed, and any sentence beginning "Before, …".

Before (12 lines, two entries) → after (6 lines, one):

```markdown
- **A damaged save of a cup tournament brought the server down instead of being
  refused.** The bracket is rebuilt from the frozen seeding on every read, and
  that rebuild trusted the bracket size and the seed list it was given — so a
  file whose two no longer agreed (a truncated write, a hand edit) crashed the
  moment anything looked at that tournament, taking the others with it. …
- **The rest of a damaged save could still bring the server down…** The same
  check now also refuses a file whose players share a registration id or a
  tournament number, whose rounds are not numbered in order, or whose boards …
```

```markdown
- **A damaged save file could bring the server down**, taking every other
  tournament with it, or quietly produce standings for a tournament nobody
  played. Saves are now checked when they are loaded, exactly as imported ones
  are, and a file that fails appears in the picker with the reason it could not
  be opened, untouched. A file with no format version at all, or carrying a key
  this version does not recognise, is now refused rather than read anyway.
```

Merge bullets that a referee would meet as one situation — successive hardenings
of the same area, an unreadable backup directory and an unreadable data
directory. Keep separate anything they'd hit on different days.

Condense a *sentence appended to an existing feature entry* the same way: when a
session refines an unreleased feature, it tends to bolt a paragraph onto that
feature's description. One clause, or nothing.

## 4. Structure and order

Sections stay `### Added` / `### Fixed` / `### Changed` / `### Removed`, in that
order, omitting the empty ones. Do not invent others — `Internal` exists in
v1.1.0 and was a mistake; internal work is not changelog material.

Within a section, lead with what matters most to a referee: security fixes
first in `Fixed`, then bugs that could break a running tournament, then
cosmetic and infrastructural ones. `Added` leads with features, ends with
polish. Chronological order is never the right order.

The compatibility note above the sections says which older saves this version
opens, and under what condition. Keep it, at three or four lines.

## 5. Finish

- `git diff --stat` — CHANGELOG.md and nothing else.
- Re-read the Unreleased section start to finish. It should read as one voice.
- Commit it alone, and say in the message which entries were **deleted** and
  why, since that is the part that isn't recoverable from the diff at a glance.
- Report the deletions to Robin explicitly rather than only the line count; he
  is the one who decides whether an entry was really not news.
