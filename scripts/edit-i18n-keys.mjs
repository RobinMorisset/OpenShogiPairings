#!/usr/bin/env node
// Add, remove or rename keys across *every* i18n locale catalogue at once.
//
// The catalogues in frontend/src/lib/i18n/locales/*.json must all expose the
// identical key tree (`check-i18n-keys.mjs` blocks the commit otherwise), so
// every key edit is really nine edits that have to agree. Done by hand — nine
// files, a position to match in each — the two failure modes are forgetting a
// locale and, worse, rewriting a catalogue with a serializer whose formatting
// differs, which reflows all ~700 lines and buries the real change. The parity
// checker catches the first and is blind to the second.
//
// So the edit goes through here instead: every locale is edited in memory,
// nothing is written unless *all* of them succeed and the result still has
// matching key trees, and a catalogue that would not survive the round-trip
// byte-for-byte is refused rather than reformatted.
//
// Usage:
//   edit-i18n-keys.mjs apply <ops.json|->   # add/remove/rename, see below
//   edit-i18n-keys.mjs remove <dotted.path>...
//   edit-i18n-keys.mjs rename <dotted.path> <newLeafName>
//
// The ops file is a JSON array; `add` carries the translations, so it is the
// one that needs a file rather than argv:
//
//   [
//     { "op": "add", "path": "settings", "after": "addExemptClub",
//       "values": {
//         "en": { "fooTitle": "Foo", "fooDesc": "…" },
//         "fr": { "fooTitle": "Foo", "fooDesc": "…" },
//         …one entry per locale, same keys in each…
//       } },
//     { "op": "remove", "path": "settings.staleKey" },
//     { "op": "rename", "path": "settings.oldName", "to": "newName" }
//   ]
//
// `path` on `add` names the *containing* group ("" for the top level) and
// `after` the sibling to insert behind (omitted = append at the end); on
// `remove`/`rename` it names the key itself. A value may be a nested object, so
// a whole new group is one `add`.

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const localesDir = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "frontend",
  "src",
  "lib",
  "i18n",
  "locales",
);

function die(message) {
  console.error(`edit-i18n-keys: ${message}`);
  process.exit(1);
}

/** The canonical on-disk form: 2-space JSON, unescaped UTF-8, trailing newline. */
function serialize(data) {
  return `${JSON.stringify(data, null, 2)}\n`;
}

function isGroup(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/**
 * Every catalogue, keyed by locale.
 *
 * A file that does not already round-trip through `serialize` is refused: the
 * tool would silently reformat it, turning a two-line change into a whole-file
 * diff that no reviewer reads.
 */
function loadCatalogues() {
  const files = readdirSync(localesDir)
    .filter((name) => name.endsWith(".json"))
    .sort();
  if (files.length === 0) die(`no catalogues found in ${localesDir}`);

  const catalogues = new Map();
  for (const file of files) {
    const source = readFileSync(join(localesDir, file), "utf8");
    let data;
    try {
      data = JSON.parse(source);
    } catch (error) {
      die(`${file}: ${error.message}`);
    }
    if (serialize(data) !== source) {
      die(
        `${file} is not in the canonical 2-space JSON form this tool writes — ` +
          `editing it would reformat the whole file. Reformat it first, deliberately.`,
      );
    }
    catalogues.set(file.replace(/\.json$/, ""), data);
  }
  return catalogues;
}

/** The group a dotted path names ("" = the root). Dies if it is missing or a string. */
function groupAt(root, path, locale) {
  if (path === "") return root;
  let node = root;
  for (const part of path.split(".")) {
    if (!isGroup(node) || !(part in node)) die(`${locale}: no such path "${path}"`);
    node = node[part];
  }
  if (!isGroup(node)) die(`${locale}: "${path}" is a message, not a group of keys`);
  return node;
}

/** Split "a.b.leaf" into ["a.b", "leaf"]; a bare "leaf" gives ["", "leaf"]. */
function splitLeaf(path) {
  const cut = path.lastIndexOf(".");
  return cut === -1 ? ["", path] : [path.slice(0, cut), path.slice(cut + 1)];
}

/** Replace `group`'s contents with `entries`, preserving the object's identity. */
function replaceContents(group, entries) {
  for (const key of Object.keys(group)) delete group[key];
  Object.assign(group, entries);
}

/** Flattened dotted key paths, for the parity check (mirrors check-i18n-keys.mjs). */
function collectKeys(node, prefix, out) {
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (isGroup(value)) collectKeys(value, path, out);
    else out.add(path);
  }
}

function applyAdd(catalogues, op) {
  const { path = "", after, values } = op;
  if (!isGroup(values)) die(`add "${path}": needs a "values" object, one entry per locale`);

  // The locale set must match exactly. Silently skipping a missing locale is
  // the failure this tool exists to prevent, and an unknown one is a typo.
  const expected = [...catalogues.keys()].sort();
  const supplied = Object.keys(values).sort();
  if (expected.join(",") !== supplied.join(",")) {
    die(
      `add "${path}": translations cover [${supplied}] but the catalogues are ` +
        `[${expected}] — every locale must be supplied`,
    );
  }

  for (const [locale, data] of catalogues) {
    const group = groupAt(data, path, locale);
    const additions = values[locale];
    if (!isGroup(additions)) die(`add "${path}": ${locale}'s entry is not an object`);
    for (const key of Object.keys(additions)) {
      if (key in group) {
        die(`add "${path}": ${locale} already defines "${key}" — remove or rename it first`);
      }
    }
    if (after === undefined) {
      Object.assign(group, additions);
      continue;
    }
    if (!(after in group)) die(`add "${path}": ${locale} has no anchor key "${after}"`);
    const rebuilt = {};
    for (const [key, value] of Object.entries(group)) {
      rebuilt[key] = value;
      if (key === after) Object.assign(rebuilt, additions);
    }
    replaceContents(group, rebuilt);
  }
}

function applyRemove(catalogues, op) {
  const [parentPath, leaf] = splitLeaf(op.path);
  for (const [locale, data] of catalogues) {
    const group = groupAt(data, parentPath, locale);
    if (!(leaf in group)) die(`remove "${op.path}": ${locale} has no such key`);
    delete group[leaf];
  }
}

function applyRename(catalogues, op) {
  const [parentPath, leaf] = splitLeaf(op.path);
  if (!op.to || op.to.includes(".")) {
    die(`rename "${op.path}": "to" must be a single new leaf name, not a path`);
  }
  for (const [locale, data] of catalogues) {
    const group = groupAt(data, parentPath, locale);
    if (!(leaf in group)) die(`rename "${op.path}": ${locale} has no such key`);
    if (op.to in group) die(`rename "${op.path}": ${locale} already defines "${op.to}"`);
    // Rebuild so the key keeps its position rather than jumping to the end.
    replaceContents(
      group,
      Object.fromEntries(
        Object.entries(group).map(([key, value]) => [key === leaf ? op.to : key, value]),
      ),
    );
  }
}

const HANDLERS = { add: applyAdd, remove: applyRemove, rename: applyRename };

function run(ops) {
  if (!Array.isArray(ops) || ops.length === 0) die("no operations to apply");
  const catalogues = loadCatalogues();

  for (const op of ops) {
    const handler = HANDLERS[op.op];
    if (!handler) die(`unknown operation "${op.op}" (expected add, remove or rename)`);
    if (typeof op.path !== "string" && !(op.op === "add" && op.path === undefined)) {
      die(`operation "${op.op}" needs a "path"`);
    }
    handler(catalogues, op);
  }

  // The invariant the commit hook enforces, checked here so a bad edit costs
  // nothing rather than leaving nine files to hand-repair.
  let reference = null;
  for (const [locale, data] of catalogues) {
    const keys = new Set();
    collectKeys(data, "", keys);
    if (reference === null) {
      reference = { locale, keys };
      continue;
    }
    const missing = [...reference.keys].filter((k) => !keys.has(k));
    const extra = [...keys].filter((k) => !reference.keys.has(k));
    if (missing.length || extra.length) {
      die(
        `key trees diverged, nothing written — ${locale} vs ${reference.locale}: ` +
          `${missing.length} missing [${missing.slice(0, 5)}], ` +
          `${extra.length} extra [${extra.slice(0, 5)}]`,
      );
    }
  }

  for (const [locale, data] of catalogues) {
    writeFileSync(join(localesDir, `${locale}.json`), serialize(data), "utf8");
  }
  console.log(
    `edit-i18n-keys: applied ${ops.length} operation(s) to ${catalogues.size} catalogues ` +
      `(${reference.keys.size} keys each)`,
  );
}

const [command, ...rest] = process.argv.slice(2);
switch (command) {
  case "apply": {
    if (rest.length !== 1) die("usage: edit-i18n-keys.mjs apply <ops.json|->");
    const source = readFileSync(rest[0] === "-" ? 0 : rest[0], "utf8");
    let ops;
    try {
      ops = JSON.parse(source);
    } catch (error) {
      die(`ops file: ${error.message}`);
    }
    run(ops);
    break;
  }
  case "remove":
    if (rest.length === 0) die("usage: edit-i18n-keys.mjs remove <dotted.path>...");
    run(rest.map((path) => ({ op: "remove", path })));
    break;
  case "rename":
    if (rest.length !== 2) die("usage: edit-i18n-keys.mjs rename <dotted.path> <newLeafName>");
    run([{ op: "rename", path: rest[0], to: rest[1] }]);
    break;
  default:
    die(
      "usage: edit-i18n-keys.mjs apply <ops.json|-> | remove <path>... | rename <path> <newLeaf>",
    );
}
