import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import path from "node:path";
import {
  brotliCompressSync,
  constants,
  gzipSync,
} from "node:zlib";

const dist = new URL("../dist/", import.meta.url);
const compressible = new Set([".css", ".html", ".js", ".json", ".svg"]);

async function filesBelow(directory) {
  const entries = await fs.readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  const files = [];
  for (const entry of entries) {
    const child = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await filesBelow(child));
    else if (entry.isFile()) files.push(child);
  }
  return files;
}

const indexPath = new URL("index.html", dist);
const index = await fs.readFile(indexPath, "utf8");
const inlineScripts = [...index.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g)]
  .filter((match) => !/\bsrc\s*=/.test(match[0]))
  .map((match) => match[1])
  .filter((script) => script.length > 0);

if (inlineScripts.length !== 1) {
  throw new Error(`expected one inline theme bootstrap, found ${inlineScripts.length}`);
}

const themeHash = createHash("sha256").update(inlineScripts[0], "utf8").digest("base64");
await fs.writeFile(new URL(".csp-theme-hash", dist), themeHash);

for (const file of await filesBelow(dist.pathname)) {
  if (!compressible.has(path.extname(file))) continue;
  const input = await fs.readFile(file);
  if (input.length < 256) continue;

  const brotli = brotliCompressSync(input, {
    params: {
      [constants.BROTLI_PARAM_MODE]: constants.BROTLI_MODE_TEXT,
      [constants.BROTLI_PARAM_QUALITY]: 11,
    },
  });
  const gzip = gzipSync(input, { level: 9, mtime: 0 });

  if (brotli.length < input.length) await fs.writeFile(`${file}.br`, brotli);
  if (gzip.length < input.length) await fs.writeFile(`${file}.gz`, gzip);
}
