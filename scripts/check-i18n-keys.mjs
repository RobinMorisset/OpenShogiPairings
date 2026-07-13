#!/usr/bin/env node
// Fail if the i18n locale files don't all define the exact same set of keys.
//
// Every locale in frontend/src/lib/i18n/locales/*.json must expose an
// identical key tree (nested objects are compared by their flattened dotted
// paths). A missing or extra key in any locale means the UI would fall back to
// a raw key string at runtime, so we block the commit instead.

import { readdirSync, readFileSync } from "node:fs";
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

// Collect the flattened dotted key paths of a (possibly nested) object.
function collectKeys(obj, prefix, out) {
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      collectKeys(value, path, out);
    } else {
      out.add(path);
    }
  }
}

const files = readdirSync(localesDir)
  .filter((name) => name.endsWith(".json"))
  .sort();

if (files.length < 2) {
  // Nothing to cross-check.
  process.exit(0);
}

const keysByFile = new Map();
for (const file of files) {
  const parsed = JSON.parse(readFileSync(join(localesDir, file), "utf8"));
  const keys = new Set();
  collectKeys(parsed, "", keys);
  keysByFile.set(file, keys);
}

// Use the union of all keys as the reference set and report, per file, which
// keys it is missing.
const allKeys = new Set();
for (const keys of keysByFile.values()) {
  for (const key of keys) {
    allKeys.add(key);
  }
}

let ok = true;
for (const [file, keys] of keysByFile) {
  const missing = [...allKeys].filter((key) => !keys.has(key)).sort();
  if (missing.length > 0) {
    ok = false;
    console.error(`\n${file} is missing ${missing.length} key(s):`);
    for (const key of missing) {
      console.error(`  ${key}`);
    }
  }
}

if (!ok) {
  console.error(
    "\ni18n locale files must define an identical set of keys. " +
      "Add the missing keys above (or remove the extras from the other locales).",
  );
  process.exit(1);
}
