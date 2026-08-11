---
name: i18n-keys
description: Add, remove or rename user-facing strings in this repo's nine locale catalogues (frontend/src/lib/i18n/locales/*.json) via scripts/edit-i18n-keys.mjs. Use whenever a change touches UI text — a new setting, checkbox, button, label, placeholder, tooltip, error message or rule name — or removes/renames one, since every key must exist in all nine languages or the commit is blocked. Also covers what to do when a new ts-rs enum variant needs a label.
---

# Editing the locale catalogues

The UI ships in **nine languages**: `en`, `fr`, `de`, `ja`, `pl`, `ru`, `sk`,
`uk`, `be` (`frontend/src/lib/i18n/locales/*.json`). They must all define the
*identical* key tree — `scripts/check-i18n-keys.mjs` runs in the pre-commit hook
and in CI and blocks anything else.

So one new string is nine edits that have to agree, at the same position in each
file. **Use the tool. Do not hand-edit the JSON, and do not write a one-off
script** — a serializer whose formatting differs by one space reflows all ~700
lines of every catalogue and buries the real change in the diff. The parity
checker cannot see that; `edit-i18n-keys.mjs` refuses it.

## The tool

```bash
node scripts/edit-i18n-keys.mjs remove settings.staleKey
node scripts/edit-i18n-keys.mjs rename settings.oldName newName
node scripts/edit-i18n-keys.mjs apply <ops.json>
```

It edits every locale in memory, checks the resulting key trees still match, and
writes **only if everything succeeded** — a forgotten locale, a missing anchor,
or keys that disagree between languages aborts with nothing written.

## Adding keys

`add` carries the translations, so it goes through an ops file. Write it to your
scratchpad directory, never into the repo:

```json
[
  { "op": "add", "path": "settings", "after": "addExemptClub",
    "values": {
      "en": { "fooTitle": "Foo protection", "fooDesc": "Avoid …" },
      "fr": { "fooTitle": "Protection de foo", "fooDesc": "Évite …" },
      "de": { … }, "ja": { … }, "pl": { … },
      "ru": { … }, "sk": { … }, "uk": { … }, "be": { … }
    } }
]
```

- `path` is the **containing group** (`""` for the top level); on `remove` and
  `rename` it is the full path of the key itself.
- `after` is the sibling to insert behind — use it, so the new keys land next to
  the feature they belong to instead of at the end of the group. Omit it only
  when appending is genuinely right.
- A value may be a nested object, so a whole new group is one `add`.
- All nine locales are mandatory and must carry the same keys.

## Writing the translations

This is the part the tool cannot do, and the part worth the care:

- **Translate properly. Never leave English (or a placeholder) in the other
  eight catalogues** — the parity checker only counts keys, so a stub ships
  silently and looks translated.
- **Read the neighbouring keys first** and reuse their vocabulary. Terminology is
  established per language (e.g. FR *appariement*, DE *Paarung*, JA 組み合わせ
  for pairing; DE *Vereinsschutz*, RU *Защита клубов* for club protection). When
  a feature mirrors an existing one, mirror its phrasing in every language.
- Match the existing punctuation conventions — FR uses a narrow space before
  `: ; ? !`, JA uses `、。` and full-width parens.
- Messages with `{count, plural, …}` need the arms the *language* requires, not
  just `other`; `messages.test.ts` checks this and explains why.

## Where the keys are consumed

Svelte reads them as `$_("settings.fooTitle")`. `keyUsage.test.ts` checks both
directions — a key no source asks for is reported dead, and a key the code asks
for with no catalogue entry fails. Its `DYNAMIC_SITES` expands the ts-rs unions
(`RuleId`, `ScopeReason`, `SitoutValue`, the tie-break codes), so **a new enum
variant added in Rust needs its label key here** or the test fails naming it.
That is the usual reason this skill is needed alongside a backend change.

## Verify

```bash
node scripts/check-i18n-keys.mjs
cd frontend && npm test && npm run check
```

`git diff --stat` on the catalogues is the other tell: a clean edit is a handful
of added/removed lines per file. Hundreds of changed lines means something
reformatted them.
