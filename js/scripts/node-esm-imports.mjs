// ubrn emits bundler-style relative imports (`from './wallet_core'`). Node ESM needs the
// extension, so add `.js` where a relative specifier has none. Idempotent; run after `ubrn build`.
import { readdir, readFile, writeFile } from "node:fs/promises";

const dir = new URL("../src/generated/", import.meta.url);
for (const name of await readdir(dir)) {
  if (!name.endsWith(".ts") || name.endsWith(".d.ts")) continue;
  const file = new URL(name, dir);
  const source = await readFile(file, "utf8");
  const fixed = source.replace(/(from\s+(["'])\.\/[A-Za-z0-9_-]+)\2/g, "$1.js$2");
  if (fixed !== source) await writeFile(file, fixed);
}
