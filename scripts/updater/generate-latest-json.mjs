// Build the Tauri v2 updater manifest (latest.json) from the signed release artifacts.
//
// The in-app updater (configured in tauri.conf.json) fetches this file from the GitHub
// "latest" release, compares `version` to the running app, and — if newer — downloads the
// per-platform `url` and verifies it against the `signature` (produced by the signing key).
//
// Usage: node generate-latest-json.mjs <artifacts-dir> <tag>
//   <artifacts-dir>  directory containing the merged release assets (incl. *.sig files)
//   <tag>            the git tag, e.g. v0.0.5
//
// Writes <artifacts-dir>/latest.json. Only macOS (.app.tar.gz) and Windows (.nsis.zip)
// support Tauri auto-update; Linux (deb/rpm) is installed manually and is skipped.

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const REPO = "ksg98/fastaf";

const dir = process.argv[2];
const tag = process.argv[3];
if (!dir || !tag) {
  console.error("Usage: node generate-latest-json.mjs <artifacts-dir> <tag>");
  process.exit(1);
}

const version = tag.replace(/^v/, "");
const baseUrl = `https://github.com/${REPO}/releases/download/${tag}`;
const urlFor = (name) => `${baseUrl}/${encodeURIComponent(name)}`;
const archOf = (name) => (/aarch64|arm64/i.test(name) ? "aarch64" : "x86_64");

const platforms = {};
for (const file of readdirSync(dir)) {
  let os;
  if (file.endsWith(".app.tar.gz.sig")) os = "darwin";
  else if (file.endsWith(".nsis.zip.sig")) os = "windows";
  else continue;

  const artifact = file.slice(0, -".sig".length);
  const signature = readFileSync(join(dir, file), "utf8").trim();
  platforms[`${os}-${archOf(file)}`] = { signature, url: urlFor(artifact) };
}

if (Object.keys(platforms).length === 0) {
  console.error("No updater signatures (*.sig) found — refusing to write an empty latest.json");
  process.exit(1);
}

const manifest = {
  version,
  pub_date: new Date().toISOString(),
  platforms,
};

writeFileSync(join(dir, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log("Generated latest.json:");
console.log(JSON.stringify(manifest, null, 2));
