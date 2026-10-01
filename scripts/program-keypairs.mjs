#!/usr/bin/env node
// Derives a release's Eco-prefixed program keypairs from the release secret.
//
//   PROGRAM_KEYPAIR_SECRET=<hex> node program-keypairs.mjs <version> <out-dir> <program>...
//
// Writes <out-dir>/<program>-keypair.json (Solana keypair files) and prints
// "<program> <address>". A keypair is a function of the secret, the version and the
// program's lib name only, so CI and the deployer derive identical keys and never
// transfer them. Anyone holding the secret can deploy at a release's addresses.
// Zero dependencies; requires Node 22.

import { createHmac, createPrivateKey, createPublicKey } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const ADDRESS_PREFIX = "Eco";
// Changing the domain re-keys every release; add a new version instead of editing it.
const DOMAIN = "eco-routes-svm/program-keypair/v1";
const MIN_SECRET_BYTES = 32;
const SECRET_VARIABLE = "PROGRAM_KEYPAIR_SECRET";
const USAGE = "usage: program-keypairs.mjs <version> <out-dir> <program>...";
const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
// RFC 8410 PKCS#8 wrapping of a raw 32-byte Ed25519 seed.
const ED25519_PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
// https://semver.org/#is-there-a-suggested-regular-expression-regex-to-check-a-semver-string
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;
// One spelling per program (its crate lib name), so a hyphenated alias cannot derive a
// second keypair.
const PROGRAM_NAME = /^[a-z0-9_]+$/;

export const parseSecret = (hex) => {
  const trimmed = hex.trim();
  if (!/^(?:[0-9a-fA-F]{2})+$/.test(trimmed)) {
    throw new Error(`${SECRET_VARIABLE} must be hex-encoded`);
  }
  const secret = Buffer.from(trimmed, "hex");
  if (secret.length < MIN_SECRET_BYTES) {
    throw new Error(`${SECRET_VARIABLE} must be at least ${MIN_SECRET_BYTES} bytes, got ${secret.length}`);
  }

  return secret;
};

export const parseVersion = (version) => {
  if (!SEMVER.test(version)) {
    throw new Error(`version ${JSON.stringify(version)} must be semver without a "v" prefix`);
  }

  return version;
};

export const parseProgram = (program) => {
  if (!PROGRAM_NAME.test(program)) {
    throw new Error(`program ${JSON.stringify(program)} must be a crate lib name (a-z, 0-9, _)`);
  }

  return program;
};

// The first candidate whose address starts with ADDRESS_PREFIX; about 57,000 attempts.
export const derive = (secret, version, program) =>
  attempts()
    .map((attempt) => keypair(seed(secret, version, program, attempt)))
    .find(({ address }) => address.startsWith(ADDRESS_PREFIX));

function* attempts() {
  for (let attempt = 0n; ; attempt++) {
    yield attempt;
  }
}

// NUL-separated, so no two (version, program) pairs share a message.
const seed = (secret, version, program, attempt) => {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64LE(attempt);

  return [DOMAIN, version, program]
    .reduce((mac, part) => mac.update(part).update(Buffer.of(0)), createHmac("sha256", secret))
    .update(counter)
    .digest();
};

// `bytes` is the Solana keypair-file layout: the 32-byte seed, then the public key.
const keypair = (seed) => {
  const privateKey = createPrivateKey({
    key: Buffer.concat([ED25519_PKCS8_PREFIX, seed]),
    format: "der",
    type: "pkcs8",
  });
  const publicKey = createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);

  return { address: base58(publicKey), bytes: Buffer.concat([seed, publicKey]) };
};

const base58 = (bytes) => {
  const zeros = bytes.findIndex((byte) => byte !== 0);
  const leading = "1".repeat(zeros === -1 ? bytes.length : zeros);

  return leading + digits(BigInt(`0x${bytes.toString("hex")}`));
};

const digits = (value) => (value === 0n ? "" : digits(value / 58n) + BASE58[Number(value % 58n)]);

const writeKeypair = (outDir, program, { bytes }) =>
  writeFileSync(join(outDir, `${program}-keypair.json`), JSON.stringify([...bytes]), { mode: 0o600 });

const main = ([version, outDir, ...programs]) => {
  if (!version || !outDir || programs.length === 0) {
    throw new Error(USAGE);
  }
  const secret = parseSecret(
    process.env[SECRET_VARIABLE] ?? (() => { throw new Error(`${SECRET_VARIABLE} must be set`); })(),
  );
  const parsedVersion = parseVersion(version);
  const parsedPrograms = programs.map(parseProgram);

  mkdirSync(outDir, { recursive: true });
  parsedPrograms.forEach((program) => {
    const programKeypair = derive(secret, parsedVersion, program);
    writeKeypair(outDir, program, programKeypair);
    console.log(`${program} ${programKeypair.address}`);
  });
};

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  Promise.resolve(process.argv.slice(2))
    .then(main)
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
