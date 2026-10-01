#!/usr/bin/env node
// Derives a release's Eco-prefixed program keypairs from the release secret.
//
//   PROGRAM_KEYPAIR_SECRET=<hex> node program-keypairs.mjs release <placeholder-dir> <out-dir> <program>...
//   PROGRAM_KEYPAIR_SECRET=<hex> node program-keypairs.mjs deploy <program-ids.json> <out-dir>
//
// A program's keypair is derived from the secret, its lib name and a seed: the hash of
// its bytecode built with the committed placeholder IDs, together with that of every
// program whose ID it compiles in. Unchanged code keeps its address; a change moves the
// program and everything that depends on it, since their bytecode embeds the new ID.
//
// `release` (CI) reads <placeholder-dir>/<cluster>/<program>.so, writes
// <out-dir>/<program>-keypair.json and prints the program-ids.json manifest.
// `deploy` re-derives the keypairs from that manifest and checks every address.
// Anyone holding the secret can deploy at a release's addresses. Zero dependencies; Node 22.

import { execFileSync } from "node:child_process";
import { createHash, createHmac, createPrivateKey, createPublicKey } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const ADDRESS_PREFIX = "Eco";
export const CLUSTERS = ["mainnet", "devnet"];
// The aggregator's members are fixed in its on-chain config at `init`, so it must move
// whenever any of them does, although it does not compile their IDs in.
export const AGGREGATOR = "aggregator_prover";
export const AGGREGATOR_MEMBERS = ["hyper_prover", "local_prover", "polymer_prover"];
// Changing the domain re-keys every program; add a new version instead of editing it.
const DOMAIN = "eco-routes-svm/program-keypair/v1";
const MIN_SECRET_BYTES = 32;
const SECRET_VARIABLE = "PROGRAM_KEYPAIR_SECRET";
const USAGE = [
  "usage: program-keypairs.mjs release <placeholder-dir> <out-dir> <program>...",
  "       program-keypairs.mjs deploy <program-ids.json> <out-dir>",
].join("\n");
const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
// RFC 8410 PKCS#8 wrapping of a raw 32-byte Ed25519 seed.
const ED25519_PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
// One spelling per program (its crate lib name), so a hyphenated alias cannot derive a
// second keypair.
const PROGRAM_NAME = /^[a-z0-9_]+$/;
const SEED = /^[0-9a-f]{64}$/;

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

export const parseProgram = (program) => {
  if (!PROGRAM_NAME.test(program)) {
    throw new Error(`program ${JSON.stringify(program)} must be a crate lib name (a-z, 0-9, _)`);
  }

  return program;
};

export const parseSeed = (seed) => {
  if (!SEED.test(seed)) {
    throw new Error(`seed ${JSON.stringify(seed)} must be 64 lowercase hex characters`);
  }

  return seed;
};

// For each program, the released programs whose IDs its bytecode depends on, directly or
// transitively: its Cargo dependencies, plus the aggregator's members. Cargo forbids
// dependency cycles, so the closure terminates.
export const dependencies = (metadata, programs) => {
  const libName = (name) => name.replaceAll("-", "_");
  const direct = Object.fromEntries(
    programs.map((program) => {
      const manifest = metadata.packages.find(({ name }) => libName(name) === program);
      if (!manifest) {
        throw new Error(`program ${program} is not a workspace package`);
      }
      const cargoDependencies = manifest.dependencies
        .filter(({ kind }) => kind === null)
        .map(({ name }) => libName(name));
      const configured = program === AGGREGATOR ? AGGREGATOR_MEMBERS : [];

      return [program, [...cargoDependencies, ...configured].filter((name) => programs.includes(name))];
    }),
  );
  const closure = (program) => direct[program].flatMap((dependency) => [dependency, ...closure(dependency)]);

  return Object.fromEntries(programs.map((program) => [program, [...new Set(closure(program))].toSorted()]));
};

// `binaries[program][cluster]` is the placeholder build's bytecode. Each seed covers the
// program and its dependencies on every cluster, since both clusters share one address.
export const seeds = (binaries, programDependencies) =>
  Object.fromEntries(
    Object.entries(programDependencies).map(([program, members]) => [
      program,
      [program, ...members]
        .toSorted()
        .reduce(
          (hash, member) =>
            CLUSTERS.reduce(
              (memberHash, cluster) => memberHash.update(sha256(binary(binaries, member, cluster))),
              hash.update(member).update(Buffer.of(0)),
            ),
          createHash("sha256"),
        )
        .digest("hex"),
    ]),
  );

// The first candidate whose address starts with ADDRESS_PREFIX; about 57,000 attempts.
export const derive = (secret, program, seed) =>
  attempts()
    .map((attempt) => keypair(candidateSeed(secret, program, seed, attempt)))
    .find(({ address }) => address.startsWith(ADDRESS_PREFIX));

const binary = (binaries, program, cluster) => {
  const bytes = binaries[program]?.[cluster];
  if (!bytes) {
    throw new Error(`missing ${cluster} placeholder build of ${program}`);
  }

  return bytes;
};

const sha256 = (bytes) => createHash("sha256").update(bytes).digest();

function* attempts() {
  for (let attempt = 0n; ; attempt++) {
    yield attempt;
  }
}

// NUL-separated, so no two (program, seed) pairs share a message.
const candidateSeed = (secret, program, seed, attempt) => {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64LE(attempt);

  return [DOMAIN, program, seed]
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

const release = (secret, [placeholderDir, outDir, ...programs]) => {
  if (!placeholderDir || !outDir || programs.length === 0) {
    throw new Error(USAGE);
  }
  const released = programs.map(parseProgram);
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { encoding: "utf8" }),
  );
  const binaries = Object.fromEntries(
    released.map((program) => [
      program,
      Object.fromEntries(
        CLUSTERS.map((cluster) => [cluster, readFileSync(join(placeholderDir, cluster, `${program}.so`))]),
      ),
    ]),
  );
  const programSeeds = seeds(binaries, dependencies(metadata, released));
  const keypairs = released.map((program) => [program, derive(secret, program, programSeeds[program])]);

  mkdirSync(outDir, { recursive: true });
  keypairs.forEach(([program, programKeypair]) => writeKeypair(outDir, program, programKeypair));
  console.log(
    JSON.stringify(
      Object.fromEntries(
        keypairs.map(([program, { address }]) => [program, { address, seed: programSeeds[program] }]),
      ),
      null,
      2,
    ),
  );
};

const deploy = (secret, [manifestPath, outDir]) => {
  if (!manifestPath || !outDir) {
    throw new Error(USAGE);
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const keypairs = Object.entries(manifest).map(([program, { address, seed }]) => {
    const programKeypair = derive(secret, parseProgram(program), parseSeed(seed));
    if (programKeypair.address !== address) {
      throw new Error(`${program}: derived ${programKeypair.address}, manifest has ${address}; wrong secret?`);
    }

    return [program, programKeypair];
  });

  mkdirSync(outDir, { recursive: true });
  keypairs.forEach(([program, programKeypair]) => {
    writeKeypair(outDir, program, programKeypair);
    console.log(`${program} ${programKeypair.address}`);
  });
};

const COMMANDS = { release, deploy };

const main = ([command, ...args]) => {
  const run = COMMANDS[command];
  if (!run) {
    throw new Error(USAGE);
  }
  const secret = parseSecret(
    process.env[SECRET_VARIABLE] ??
      (() => {
        throw new Error(`${SECRET_VARIABLE} must be set`);
      })(),
  );

  run(secret, args);
};

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  Promise.resolve(process.argv.slice(2))
    .then(main)
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
