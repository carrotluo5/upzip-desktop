// Keep gui/src-tauri/binaries/ in sync with binaries/ (the single source of truth).
//
// Tauri bundles gui/src-tauri/binaries/** as app resources, while the engine's dev
// fallback resolves binaries/ from the repo root. Two hand-maintained copies could
// drift silently, so this runs before every `tauri build` (see gui/package.json and
// gui/src-tauri/tauri.conf.json `beforeBuildCommand`).
//
// It only mirrors files: it does NOT download, verify or re-sign them. Provenance,
// version and hashes of the vendored 7-Zip binaries are documented in
// binaries/README.md. Missing sources are a hard error (exit code 1).
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEST = "gui/src-tauri/binaries";

const PAIRS = [
  ["binaries/mac/7zz", DEST + "/mac/7zz"],
  ["binaries/win/7z.exe", DEST + "/win/7z.exe"],
  ["binaries/win/7z.dll", DEST + "/win/7z.dll"],
  ["binaries/7z-LICENSE.txt", DEST + "/7z-LICENSE.txt"],
];

const sha256 = (p) => createHash("sha256").update(readFileSync(p)).digest("hex");

let copied = 0;
let missing = 0;
for (const [src, dst] of PAIRS) {
  const s = join(root, src);
  const d = join(root, dst);
  if (!existsSync(s)) {
    console.log(`missing source: ${src}`);
    missing += 1;
    continue;
  }
  const identical =
    existsSync(d) && statSync(s).size === statSync(d).size && sha256(s) === sha256(d);
  if (identical) {
    console.log(`ok    ${dst}`);
    continue;
  }
  mkdirSync(dirname(d), { recursive: true });
  copyFileSync(s, d);
  if (!src.endsWith(".txt")) {
    chmodSync(d, 0o755); // copyFileSync does not preserve the executable bit
  }
  copied += 1;
  console.log(`sync  ${src} -> ${dst}`);
}

console.log(`sync-binaries: ${copied} updated, ${missing} missing source(s)`);
if (missing > 0) process.exitCode = 1;
