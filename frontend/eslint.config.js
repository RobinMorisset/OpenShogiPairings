import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';
import svelteConfig from './svelte.config.js';

export default ts.config(
  {
    ignores: [
      'dist/',
      'src-tauri/',
      // ts-rs writes these from the Rust types; lint findings here would have
      // to be fixed in the generator, not the checked-in output.
      'src/lib/generated/',
    ],
  },
  js.configs.recommended,
  ts.configs.recommended,
  svelte.configs.recommended,
  {
    languageOptions: {
      globals: { ...globals.browser },
    },
    rules: {
      // Deliberately off: every Map/Set we build is filled inside a `$derived.by`
      // (or a plain helper) and returned, never mutated afterwards, so the
      // reactive variants would buy nothing. The rule can't tell that apart from
      // a long-lived mutated collection, and fired only false positives here.
      // Turn it back on if a component ever keeps a Map/Set in `$state`.
      "svelte/prefer-svelte-reactivity": "off",
    },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: {
      parserOptions: {
        parser: ts.parser,
        svelteConfig,
      },
    },
  },
  {
    files: ['*.config.js', '*.config.ts'],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
);
