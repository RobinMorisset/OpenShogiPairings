// Pick a CSV roster file and hand its raw text to the caller.
//
// The parsing itself (column detection, FESA enrichment, validation) lives in
// the server (`osp-core`'s `parse_players_csv`), so there is a single tested
// implementation of the format. This module only covers picking the file, which
// is inherently client-side (a native dialog under Tauri, a hidden <input> in
// the browser).

import { isTauri } from "./platform";

/** Filters offered in the native/browser file picker. */
const CSV_FILTERS = [{ name: "CSV", extensions: ["csv"] }];

/**
 * Prompt the user to pick a CSV file and return its raw text, or `null` if
 * the dialog was cancelled.
 */
export function pickCsvFile(): Promise<string | null> {
  return isTauri() ? pickCsvViaTauri() : pickCsvViaBrowser();
}

async function pickCsvViaTauri(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { invoke } = await import("@tauri-apps/api/core");

  const selected = await open({ multiple: false, directory: false, filters: CSV_FILTERS });
  if (typeof selected !== "string") return null; // cancelled

  return invoke<string>("read_text_file", { path: selected });
}

function pickCsvViaBrowser(): Promise<string | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".csv,text/csv";
    input.style.display = "none";
    document.body.appendChild(input);

    let settled = false;
    const finish = (result: string | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(result);
    };

    input.addEventListener("change", async () => {
      const file = input.files?.[0];
      if (!file) return finish(null);
      finish(await file.text());
    });

    // There is no reliable "cancel" event for a file input, so detect it via the
    // window regaining focus without a file having been chosen.
    window.addEventListener(
      "focus",
      () => setTimeout(() => {
        if (!input.files?.length) finish(null);
      }, 300),
      { once: true },
    );

    input.click();
  });
}
