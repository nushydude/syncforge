import fs from "node:fs";

const packageJson = JSON.parse(fs.readFileSync("package.json", "utf8"));
const cargoToml = fs.readFileSync("src-tauri/Cargo.toml", "utf8");
const cargoLock = fs.readFileSync("src-tauri/Cargo.lock", "utf8");
const tauriConfig = JSON.parse(
  fs.readFileSync("src-tauri/tauri.conf.json", "utf8"),
);

const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const lockPackage = cargoLock.match(
  /name = "syncforge"\r?\nversion = "([^"]+)"/m,
)?.[1];
const versions = {
  "package.json": packageJson.version,
  "src-tauri/Cargo.toml": cargoVersion,
  "src-tauri/Cargo.lock": lockPackage,
  "src-tauri/tauri.conf.json": tauriConfig.version,
};

const missing = Object.entries(versions).filter(([, version]) => !version);
const distinct = new Set(Object.values(versions));
if (missing.length > 0 || distinct.size !== 1) {
  console.error("SyncForge version sources must all exist and match:");
  for (const [file, version] of Object.entries(versions)) {
    console.error(`  ${file}: ${version ?? "missing"}`);
  }
  process.exit(1);
}

console.log(`Version sources match: ${[...distinct][0]}`);
