#!/usr/bin/env node
// Fail if the frontend reads a CSS custom property that nothing defines.
//
// `var(--typo)` is silent in every direction: the browser reports nothing, the
// bundler reports nothing, svelte-check reports nothing, and the rule does not
// merely lose its value — an unresolvable `var()` makes the whole declaration
// "invalid at computed-value time", so the property falls back to inherit or to
// its initial value. `border: 1px solid var(--color-border)` with no such
// variable is not a default-coloured border, it is *no border at all*. That is
// how the standings' long-game bracket spent months rendering nothing while
// looking perfectly correct in review, and how five muted labels across
// TeamsPanel, RoundView and Combobox were quietly not muted.
//
// The rule: every `var(--name)` must resolve to one of
//   1. a variable app.css defines (the shared palette, both themes), or
//   2. a variable the same file defines — in its own CSS, or in an inline
//      `style="--name: …"` it builds in script (ResultSheets does this for the
//      slip geometry it computes), or
//   3. a `var(--name, fallback)` that says what to use when nobody sets it.
//
// The third is also how a file legitimately reads a variable *another* file
// sets on it: TournamentSettingsView's grid uses `var(--col-min, 20rem)` and
// each settings section overrides `--col-min` for its own column width. Writing
// the fallback is what makes that contract visible at the point of use, and is
// why "defined in some other component" is deliberately not enough here.
//
// Same-file is a proxy for "in scope": whether a definition on `.standings`
// actually reaches the `.pin-id` that reads it is a cascade question no text
// scan can answer. It catches the class of bug that matters — a name that
// exists nowhere at all.

import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = join(repoRoot, "frontend", "src");
const globalCssPath = join(srcDir, "app.css");

// .ts as well as .svelte and .css: publicExport.ts and ResultSheets.svelte both
// build stylesheets as template strings, and a dead variable is exactly as dead
// there.
const SCANNED = /\.(svelte|ts|css)$/;

// A definition: `--name:` — in a rule, in a `:root` block, or inside a string
// being assembled into a `style` attribute.
const DEFINITION = /--[A-Za-z0-9_-]+\s*:/g;
// A use, capturing whether a fallback follows.
const USE = /var\(\s*(--[A-Za-z0-9_-]+)\s*(,?)/g;

function collectFiles(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      collectFiles(path, out);
    } else if (SCANNED.test(entry.name)) {
      out.push(path);
    }
  }
  return out;
}

// Blank out `/* … */` comments, keeping the newlines so reported line numbers
// still match the file. Both directions matter: the long comments in this
// codebase quote CSS freely, so without this a commented-out rule could vouch
// for a variable nobody defines, and a `var()` quoted in an explanation could
// fail a check over a line that renders nothing.
function stripBlockComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, (comment) =>
    comment.replace(/[^\n]/g, " "),
  );
}

function definedIn(source) {
  const names = new Set();
  for (const [match] of source.matchAll(DEFINITION)) {
    names.add(match.slice(0, match.indexOf(":")).trim());
  }
  return names;
}

function lineOf(source, index) {
  let line = 1;
  for (let i = 0; i < index; i += 1) {
    if (source[i] === "\n") line += 1;
  }
  return line;
}

const globalNames = definedIn(stripBlockComments(readFileSync(globalCssPath, "utf8")));

const problems = [];
for (const path of collectFiles(srcDir)) {
  const source = stripBlockComments(readFileSync(path, "utf8"));
  const localNames = definedIn(source);

  for (const match of source.matchAll(USE)) {
    const [, name, fallback] = match;
    if (fallback || globalNames.has(name) || localNames.has(name)) continue;
    problems.push({
      file: relative(repoRoot, path).replaceAll("\\", "/"),
      line: lineOf(source, match.index),
      name,
    });
  }
}

if (problems.length > 0) {
  console.error(`\n${problems.length} use(s) of an undefined CSS custom property:`);
  for (const { file, line, name } of problems) {
    console.error(`  ${file}:${line}  ${name}`);
  }
  console.error(
    "\nAn unresolvable var() drops the whole declaration, so these rules do" +
      "\nnothing at all — silently. Point each at a variable app.css defines," +
      "\ndefine it in the same file, or give it a fallback: var(--name, …).",
  );
  process.exit(1);
}
