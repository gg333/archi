import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputDir = join(root, "third_party", "dependency-licenses");
const notices = [];

rmSync(outputDir, { recursive: true, force: true });
mkdirSync(outputDir, { recursive: true });

function licenseFiles(packageDir) {
  return readdirSync(packageDir)
    .filter((name) => /^(licen[cs]e|copying|notice|unlicense)([^a-z]|$)/i.test(name))
    .filter((name) => statSync(join(packageDir, name)).isFile());
}

function copyLicenses(kind, name, version, packageDir, license, source, extraFile) {
  const files = new Set(licenseFiles(packageDir));
  if (extraFile) files.add(extraFile);

  const target = join(outputDir, kind, `${name.replaceAll("/", "__")}-${version}`);
  mkdirSync(target, { recursive: true });
  if (!files.size) {
    const sourceLine = source ? `\nSource: ${source}` : "";
    writeFileSync(join(target, "LICENSE-DECLARATION.txt"), `${name} ${version}\nLicense declared by package manifest: ${license}${sourceLine}\n`);
    return target;
  }
  for (const file of [...files].sort()) {
    const source = resolve(packageDir, file);
    copyFileSync(source, join(target, file.split("/").at(-1)));
  }
  return target;
}

const metadata = JSON.parse(execFileSync("cargo", [
  "metadata",
  "--manifest-path", join(root, "src-tauri", "Cargo.toml"),
  "--format-version", "1",
  "--filter-platform", "aarch64-apple-darwin",
], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 }));

const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
const runtimeIds = new Set();
const pending = [metadata.resolve.root];
while (pending.length) {
  const id = pending.pop();
  if (runtimeIds.has(id)) continue;
  runtimeIds.add(id);
  for (const dep of nodes.get(id)?.deps ?? []) {
    if (dep.dep_kinds.some(({ kind }) => kind === null)) pending.push(dep.pkg);
  }
}
runtimeIds.delete(metadata.resolve.root);

const rustPackages = [...runtimeIds]
  .map((id) => packages.get(id))
  .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
for (const pkg of rustPackages) {
  const packageDir = dirname(pkg.manifest_path);
  copyLicenses("rust", pkg.name, pkg.version, packageDir, pkg.license, pkg.repository ?? pkg.source ?? "", pkg.license_file);
  notices.push({
    ecosystem: "Rust",
    name: pkg.name,
    version: pkg.version,
    license: pkg.license ?? "See included license file",
    source: pkg.repository ?? pkg.source ?? "",
  });
}

const jsPackageJsons = [
  "node_modules/@tauri-apps/api/package.json",
  "node_modules/@tauri-apps/plugin-dialog/package.json",
  "node_modules/@tauri-apps/plugin-notification/package.json",
  "node_modules/@crabnebula/tauri-plugin-drag/package.json",
  "node_modules/react/package.json",
  "node_modules/react-dom/package.json",
  "node_modules/.pnpm/scheduler@0.27.0/node_modules/scheduler/package.json",
];
for (const relativePath of jsPackageJsons) {
  const manifestPath = realpathSync(join(root, relativePath));
  const pkg = JSON.parse(readFileSync(manifestPath, "utf8"));
  const packageDir = dirname(manifestPath);
  const isDragPlugin = pkg.name === "@crabnebula/tauri-plugin-drag";
  const license = pkg.license ?? (isDragPlugin ? "MIT OR Apache-2.0" : "See source");
  const source = typeof pkg.repository === "string"
    ? pkg.repository
    : pkg.repository?.url ?? (isDragPlugin ? "https://github.com/crabnebula-dev/drag-rs" : "");
  const target = copyLicenses("javascript", pkg.name, pkg.version, packageDir, license, source);
  if (isDragPlugin) {
    const rustPackage = rustPackages.find(({ name }) => name === "tauri-plugin-drag");
    for (const file of licenseFiles(dirname(rustPackage.manifest_path))) {
      copyFileSync(join(dirname(rustPackage.manifest_path), file), join(target, file));
    }
  }
  notices.push({
    ecosystem: "JavaScript",
    name: pkg.name,
    version: pkg.version,
    license,
    source,
  });
}

const rows = notices
  .sort((a, b) => a.ecosystem.localeCompare(b.ecosystem) || a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
  .map(({ ecosystem, name, version, license, source }) => `| ${ecosystem} | ${name} | ${version} | ${license} | ${source} |`)
  .join("\n");

writeFileSync(join(root, "THIRD_PARTY_NOTICES.md"), `# Archi third-party notices

Archi includes software developed by third parties. Archi itself is not licensed under the licenses listed here. Those licenses apply only to the identified third-party components.

## 7-Zip 26.02

Archi bundles the official 7-Zip 26.02 console executable. 7-Zip is free software distributed under the GNU LGPL, with the unRAR restriction applying to portions of the code. The complete upstream license notice, full GNU LGPL 2.1 text, unRAR restriction, and exact corresponding source archive are included under \`third_party/7zip\` in this distribution.

- Project: https://www.7-zip.org/
- Binary release: https://www.7-zip.org/a/7z2602-mac.tar.xz
- Corresponding source: https://www.7-zip.org/a/7z2602-src.tar.xz
- Included source SHA-256: \`cf967c98bca02a4b8b16375f441825a8e141362f14be1969bbec8e1ca0bff9dd\`

## Other shipped dependencies

License material supplied in each upstream package is included under \`third_party/dependency-licenses\` in this distribution. When an upstream package archive contains no separate license file, its manifest license declaration and source URL are included instead.

| Ecosystem | Package | Version | License | Source |
| --- | --- | --- | --- | --- |
${rows}
`);

console.log(`Wrote notices for ${rustPackages.length} Rust and ${jsPackageJsons.length} JavaScript packages.`);
