import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";

const identity = process.env.APPLE_SIGNING_IDENTITY;
if (!identity) throw new Error("APPLE_SIGNING_IDENTITY is required for a public macOS release.");

const identities = execFileSync("security", ["find-identity", "-v", "-p", "codesigning"], { encoding: "utf8" });
if (!identities.includes(`\"${identity}\"`)) throw new Error(`Signing identity is not available in Keychain: ${identity}`);

const apiCredentials = ["APPLE_API_ISSUER", "APPLE_API_KEY", "APPLE_API_KEY_PATH"].every((name) => process.env[name]);
const appleIdCredentials = ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"].every((name) => process.env[name]);
if (!apiCredentials && !appleIdCredentials) {
  throw new Error("Set complete App Store Connect API-key credentials or Apple-ID notarization credentials.");
}
if (apiCredentials && !existsSync(process.env.APPLE_API_KEY_PATH)) {
  throw new Error(`APPLE_API_KEY_PATH does not exist: ${process.env.APPLE_API_KEY_PATH}`);
}

console.log(`Release preflight passed for ${identity}.`);
