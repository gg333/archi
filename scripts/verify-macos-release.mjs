import { execFileSync, spawnSync } from "node:child_process";
import {
  cpSync,
  createReadStream,
  existsSync,
  lstatSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { basename, join, resolve } from "node:path";
import { tmpdir } from "node:os";

const root = resolve(import.meta.dirname, "..");
const input = process.argv[2] === "--" ? process.argv[3] : process.argv[2];
const dmg = resolve(input ?? "");
const expectedTeam = process.env.APPLE_TEAM_ID ?? "37XUNJ3WYK";

if (!input || !existsSync(dmg) || !dmg.endsWith(".dmg")) {
  throw new Error("Usage: pnpm verify:macos -- /absolute/path/to/Archi_<version>_universal.dmg");
}

function output(command, args, options = {}) {
  return execFileSync(command, args, { encoding: "utf8", ...options }).trim();
}

function metadata(path) {
  const result = spawnSync("codesign", ["-dv", "--verbose=4", path], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(result.stderr || `Could not inspect ${path}`);
  return `${result.stdout}${result.stderr}`;
}

function verifySignature(path) {
  execFileSync("codesign", ["--verify", "--deep", "--strict", "--verbose=2", path], {
    stdio: "inherit",
  });
  const details = metadata(path);
  if (!details.includes("Authority=Developer ID Application:")) {
    throw new Error(`${basename(path)} is not signed with a Developer ID Application certificate.`);
  }
  if (!details.includes(`TeamIdentifier=${expectedTeam}`)) {
    throw new Error(`${basename(path)} is not signed by Apple team ${expectedTeam}.`);
  }
}

function findNamed(directory, predicate) {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name);
    const entry = lstatSync(path);
    if (predicate(name, path, entry)) return path;
    if (entry.isDirectory() && !entry.isSymbolicLink() && !name.endsWith(".app")) {
      const nested = findNamed(path, predicate);
      if (nested) return nested;
    }
  }
}

function sha256(path) {
  return new Promise((resolveHash, reject) => {
    const hash = createHash("sha256");
    createReadStream(path)
      .on("data", (chunk) => hash.update(chunk))
      .on("error", reject)
      .on("end", () => resolveHash(hash.digest("hex")));
  });
}

const packageVersion = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
const tauriVersion = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8")).version;
const cargoVersion = readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8").match(
  /^version = "([^"]+)"/m,
)?.[1];
if (!packageVersion || packageVersion !== tauriVersion || packageVersion !== cargoVersion) {
  throw new Error("package.json, Cargo.toml, and tauri.conf.json versions do not match.");
}
if (!basename(dmg).includes(packageVersion)) {
  throw new Error(`The DMG filename does not contain version ${packageVersion}.`);
}
if (output("git", ["status", "--porcelain", "--untracked-files=no"], { cwd: root })) {
  throw new Error("Tracked source files are not clean.");
}
const releaseTag = `v${packageVersion}`;
const head = output("git", ["rev-parse", "HEAD"], { cwd: root });
const tagged = output("git", ["rev-parse", `${releaseTag}^{commit}`], { cwd: root });
if (head !== tagged) throw new Error(`${releaseTag} does not point to the current commit.`);

const expected7zz = join(root, "src-tauri/binaries/7zz-universal-apple-darwin");
const checksumLine = readFileSync(join(root, "src-tauri/binaries/SHA256SUMS"), "utf8")
  .split("\n")
  .find((line) => line.trim().endsWith("7zz-universal-apple-darwin"));
if (!checksumLine || (await sha256(expected7zz)) !== checksumLine.trim().split(/\s+/)[0]) {
  throw new Error("The checked-in universal 7zz binary does not match SHA256SUMS.");
}

let mountPoint;
try {
  const plist = execFileSync("hdiutil", ["attach", "-plist", "-readonly", "-nobrowse", dmg]);
  const attached = JSON.parse(
    output("plutil", ["-convert", "json", "-o", "-", "-"], { input: plist }),
  );
  mountPoint = attached["system-entities"].find((entry) => entry["mount-point"])?.["mount-point"];
  if (!mountPoint) throw new Error("The DMG mounted without a readable volume.");

  const app = findNamed(mountPoint, (name) => name.endsWith(".app"));
  if (!app) throw new Error("The DMG does not contain an application bundle.");
  const infoPlist = join(app, "Contents/Info.plist");
  const executableName = output("plutil", [
    "-extract",
    "CFBundleExecutable",
    "raw",
    "-o",
    "-",
    infoPlist,
  ]);
  const services = JSON.parse(
    output("plutil", ["-extract", "NSServices", "json", "-o", "-", infoPlist]),
  );
  if (!Array.isArray(services) || services.length !== 5) {
    throw new Error(
      `The app bundle must advertise exactly five Finder Services; found ${services.length ?? 0}.`,
    );
  }
  if (services.some((service) => service.NSPortName !== executableName)) {
    throw new Error(
      `Every Finder Service NSPortName must match CFBundleExecutable (${executableName}).`,
    );
  }
  const executable = join(app, "Contents/MacOS", executableName);
  const sevenZip = findNamed(join(app, "Contents"), (name, _path, entry) =>
    name === "7zz" && entry.isFile(),
  );
  if (!sevenZip) throw new Error("The app bundle does not contain 7zz.");

  for (const binary of [executable, sevenZip]) {
    const architectures = output("lipo", ["-archs", binary]).split(/\s+/);
    if (!architectures.includes("arm64") || !architectures.includes("x86_64")) {
      throw new Error(`${basename(binary)} is not universal (arm64 + x86_64).`);
    }
  }

  verifySignature(app);
  verifySignature(sevenZip);
  verifySignature(dmg);

  const comparison = mkdtempSync(join(tmpdir(), "archi-7zz-"));
  try {
    const expectedCopy = join(comparison, "expected-7zz");
    const bundledCopy = join(comparison, "bundled-7zz");
    cpSync(expected7zz, expectedCopy);
    cpSync(sevenZip, bundledCopy);
    execFileSync("codesign", ["--remove-signature", expectedCopy]);
    execFileSync("codesign", ["--remove-signature", bundledCopy]);
    execFileSync("codesign", ["--force", "--sign", "-", "--identifier", "7zz", expectedCopy]);
    execFileSync("codesign", ["--force", "--sign", "-", "--identifier", "7zz", bundledCopy]);
    if ((await sha256(expectedCopy)) !== (await sha256(bundledCopy))) {
      throw new Error("The bundled 7zz payload differs from the checked-in binary.");
    }
  } finally {
    rmSync(comparison, { recursive: true, force: true });
  }

  execFileSync("xcrun", ["stapler", "validate", app], { stdio: "inherit" });
  execFileSync("xcrun", ["stapler", "validate", dmg], { stdio: "inherit" });
  execFileSync("spctl", ["--assess", "--type", "execute", "--verbose=4", app], {
    stdio: "inherit",
  });

  console.log(`Verified ${releaseTag}: ${await sha256(dmg)}  ${basename(dmg)}`);
} finally {
  if (mountPoint) execFileSync("hdiutil", ["detach", mountPoint], { stdio: "inherit" });
}
