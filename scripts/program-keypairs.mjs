#!/usr/bin/env node
// Derives a release's Eco-prefixed program keypairs from the release secret.
//
//   PROGRAM_KEYPAIR_SECRET=<hex> node program-keypairs.mjs release <placeholder-dir> <program>...
//   PROGRAM_KEYPAIR_SECRET=<hex> node program-keypairs.mjs deploy <program-ids.json> <out-dir>
//
// A program's keypair is derived from the secret, its lib name and a seed. The seed hashes
// the program's salt (program-salts.json), its bytecode built with the committed placeholder
// IDs on every cluster, and the seed of every released program whose ID it compiles in or
// configures. Unchanged code keeps its address; a change or a salt bump moves the program
// and, through their seeds, everything that depends on it.
//
// `release` (CI) reads <placeholder-dir>/<cluster>/<program>.so and prints the
// program-ids.json manifest; it writes no keys. `deploy` re-derives the keypairs from that
// manifest, checks every address, and writes <out-dir>/<program>-keypair.json.
// Anyone holding the secret can deploy at a release's addresses. Zero dependencies; Node 22.

import { execFileSync } from "node:child_process";
import { createHash, createHmac, createPrivateKey, createPublicKey } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const ADDRESS_PREFIX = "Eco";
const CLUSTERS = ["mainnet", "devnet"];
// The aggregator's members are fixed in its on-chain config at `init`, so it must move
// whenever any of them does, although it does not compile their IDs in. Keep this equal to
// the set the deployer passes to `init`.
const AGGREGATOR = "aggregator_prover";
const AGGREGATOR_MEMBERS = ["hyper_prover", "layerzero_prover", "local_prover", "polymer_prover"];
// Changing the domain re-keys every program; add a new version instead of editing it.
const DOMAIN = "eco-routes-svm/program-keypair/v1";
const MIN_SECRET_BYTES = 32;
const SECRET_VARIABLE = "PROGRAM_KEYPAIR_SECRET";
const SALTS = fileURLToPath(new URL("./program-salts.json", import.meta.url));
const USAGE = [
  "usage: program-keypairs.mjs release <placeholder-dir> <program>...",
  "       program-keypairs.mjs deploy <program-ids.json> <out-dir>",
].join("\n");
const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
// RFC 8410 PKCS#8 wrapping of a raw 32-byte Ed25519 seed.
const ED25519_PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
// One spelling per program (its crate lib name), so a hyphenated alias cannot derive a
// second keypair.
const PROGRAM_NAME = /^[a-z0-9_]+$/;
const SEED = /^[0-9a-f]{64}$/;
const NUL = Buffer.of(0);

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

// For each released program, the released programs whose IDs its bytecode depends on
// directly: Cargo dependencies, followed through unreleased workspace crates, plus the
// aggregator's members. Cargo forbids dependency cycles.
export const dependencies = (metadata, programs) => {
  const libName = (name) => name.replaceAll("-", "_");
  const workspace = new Map(metadata.packages.map((manifest) => [libName(manifest.name), manifest]));
  const cargoDependencies = (crate) =>
    workspace
      .get(crate)
      .dependencies.filter(({ kind }) => kind === null)
      .map(({ name }) => libName(name))
      .filter((name) => workspace.has(name));
  const released = (crate) =>
    cargoDependencies(crate).flatMap((name) => (programs.includes(name) ? [name] : released(name)));

  return Object.fromEntries(
    programs.map((program) => {
      if (!workspace.has(program)) {
        throw new Error(`program ${program} is not a workspace package`);
      }
      const configured = program === AGGREGATOR ? aggregatorMembers(programs) : [];

      return [program, [...new Set([...released(program), ...configured])].toSorted()];
    }),
  );
};

// `binaries[program][cluster]` is the placeholder build's bytecode; `salts[program]` defaults
// to 0. A dependency's seed, not just its bytecode, feeds each seed, so any move propagates.
export const seeds = (binaries, programDependencies, salts = {}) => {
  const cache = new Map();
  const seedOf = (program, path = []) => {
    if (path.includes(program)) {
      throw new Error(`dependency cycle: ${[...path, program].join(" -> ")}`);
    }
    if (!cache.has(program)) {
      cache.set(program, programSeed(program, [...path, program]));
    }

    return cache.get(program);
  };
  const programSeed = (program, path) => {
    const salted = createHash("sha256").update(program).update(NUL).update(u64(salt(salts, program)));
    const built = CLUSTERS.reduce(
      (hash, cluster) => hash.update(sha256(binary(binaries, program, cluster))),
      salted,
    );

    return programDependencies[program]
      .toSorted()
      .reduce(
        (hash, dependency) =>
          hash.update(dependency).update(NUL).update(Buffer.from(seedOf(dependency, path), "hex")),
        built,
      )
      .digest("hex");
  };

  return Object.fromEntries(Object.keys(programDependencies).map((program) => [program, seedOf(program)]));
};

// The first candidate whose address starts with ADDRESS_PREFIX; about 57,000 attempts.
export const derive = (secret, program, seed) =>
  attempts()
    .map((attempt) => keypair(candidateSeed(secret, program, seed, attempt)))
    .find(({ address }) => address.startsWith(ADDRESS_PREFIX));

const aggregatorMembers = (programs) => {
  const missing = AGGREGATOR_MEMBERS.filter((member) => !programs.includes(member));
  if (missing.length > 0) {
    throw new Error(`${AGGREGATOR} is released without its members: ${missing.join(", ")}`);
  }

  return AGGREGATOR_MEMBERS;
};

const salt = (salts, program) => {
  const value = salts[program] ?? 0;
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`salt of ${program} must be a non-negative integer, got ${JSON.stringify(value)}`);
  }

  return value;
};

const u64 = (value) => {
  const bytes = Buffer.alloc(8);
  bytes.writeBigUInt64LE(BigInt(value));

  return bytes;
};

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
    .reduce((mac, part) => mac.update(part).update(NUL), createHmac("sha256", secret))
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

// `mode` only applies when the file is created, so an existing file is narrowed explicitly.
const writeKeypair = (outDir, program, { bytes }) => {
  const file = join(outDir, `${program}-keypair.json`);
  writeFileSync(file, JSON.stringify([...bytes]), { mode: 0o600 });
  chmodSync(file, 0o600);
};

// Cargo runs nothing of ours here, but it has no business seeing the secret either.
const workspaceMetadata = () => {
  const { [SECRET_VARIABLE]: _secret, ...env } = process.env;

  return JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { encoding: "utf8", env }),
  );
};

const release = (secret, [placeholderDir, ...programs]) => {
  if (!placeholderDir || programs.length === 0) {
    throw new Error(USAGE);
  }
  const released = programs.map(parseProgram);
  const salts = JSON.parse(readFileSync(SALTS, "utf8"));
  const binaries = Object.fromEntries(
    released.map((program) => [
      program,
      Object.fromEntries(
        CLUSTERS.map((cluster) => [cluster, readFileSync(join(placeholderDir, cluster, `${program}.so`))]),
      ),
    ]),
  );
  const programSeeds = seeds(binaries, dependencies(workspaceMetadata(), released), salts);
  const manifest = released.map((program) => [
    program,
    {
      address: derive(secret, program, programSeeds[program]).address,
      seed: programSeeds[program],
      salt: salt(salts, program),
    },
  ]);

  console.log(JSON.stringify(Object.fromEntries(manifest), null, 2));
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

// GitHub Actions expands a missing secret to an empty string, so blank counts as unset.
const secretFromEnvironment = () => {
  const value = process.env[SECRET_VARIABLE] ?? "";
  if (value.trim() === "") {
    throw new Error(`${SECRET_VARIABLE} must be set`);
  }

  return parseSecret(value);
};

const COMMANDS = { release, deploy };

const main = ([command, ...args]) => {
  const run = COMMANDS[command];
  if (!run) {
    throw new Error(USAGE);
  }

  run(secretFromEnvironment(), args);
};

// Resolve symlinks on both sides, so a linked entry point still runs.
const invokedDirectly = () =>
  Boolean(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));

if (invokedDirectly()) {
  Promise.resolve(process.argv.slice(2))
    .then(main)
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
