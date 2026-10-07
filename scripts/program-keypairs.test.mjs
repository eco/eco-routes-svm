import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createPrivateKey, createPublicKey } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  ADDRESS_PREFIX,
  dependencies,
  derive,
  parseProgram,
  parseSecret,
  parseSeed,
  seeds,
} from "./program-keypairs.mjs";

const SCRIPT = fileURLToPath(new URL("./program-keypairs.mjs", import.meta.url));
const REPOSITORY = fileURLToPath(new URL("..", import.meta.url));
const SECRET_HEX = "07".repeat(32);
const SEED = "ab".repeat(32);
// Verified with `solana-keygen pubkey` on the written keypair file.
const PORTAL_ADDRESS = "Ecoi8woUrmkLq8PjpPFVF2k7xALmeKnAvWz2qVCkW1d7";
// Pins the seed framing: changing it re-keys every program at the next release.
const PORTAL_SEED = "5d57eacc653da7bcfd39a637cb2235dd88de5628abd089d697330c942bb16355";
const AGGREGATOR_SEED = "3efa8ccb8b05542266c1a7c557f11c83a80412050e683455df9ff90ef916fc92";
const PROGRAMS = [
  "aggregator_prover",
  "portal",
  "hyper_prover",
  "local_prover",
  "flash_fulfiller",
  "proof_helper",
  "polymer_prover",
  "layerzero_prover",
];
const CARGO_DEPENDENCIES = {
  aggregator_prover: ["portal", "anchor-lang"],
  portal: ["eco-svm-std"],
  hyper_prover: ["portal"],
  local_prover: ["portal", "flash-fulfiller"],
  flash_fulfiller: ["portal"],
  proof_helper: [],
  polymer_prover: ["portal"],
  layerzero_prover: ["portal", "eco-svm-std"],
  eco_svm_std: [],
};
const DEPENDENCIES = {
  aggregator_prover: ["hyper_prover", "layerzero_prover", "polymer_prover", "portal"],
  portal: [],
  hyper_prover: ["portal"],
  local_prover: ["flash_fulfiller", "portal"],
  flash_fulfiller: ["portal"],
  proof_helper: [],
  polymer_prover: ["portal"],
  layerzero_prover: ["portal"],
};

const secret = () => parseSecret(SECRET_HEX);

const metadata = (cargoDependencies = CARGO_DEPENDENCIES) => ({
  packages: Object.entries(cargoDependencies).map(([crate, names]) => ({
    name: crate.replaceAll("_", "-"),
    dependencies: [...names.map((name) => ({ name, kind: null })), { name: "portal", kind: "dev" }],
  })),
});

const binaries = (overrides = {}) =>
  Object.fromEntries(
    PROGRAMS.map((program) => [
      program,
      { mainnet: Buffer.from(`${program}/mainnet`), devnet: Buffer.from(`${program}/devnet`), ...overrides[program] },
    ]),
  );

const programSeeds = ({ overrides, programDependencies = DEPENDENCIES, salts } = {}) =>
  seeds(binaries(overrides), programDependencies, salts);

const moved = (changed) => {
  const base = programSeeds();

  return PROGRAMS.filter((program) => changed[program] !== base[program]).toSorted();
};

const run = (args, env, options = {}) =>
  spawnSync(process.execPath, [options.script ?? SCRIPT, ...args], {
    cwd: REPOSITORY,
    encoding: "utf8",
    env: { PATH: process.env.PATH, ...env },
  });

const temporary = () => mkdtempSync(join(tmpdir(), "program-keypairs-"));

test("derive is deterministic and Eco-prefixed", () => {
  const { address } = derive(secret(), "portal", SEED);

  assert.equal(address, PORTAL_ADDRESS);
  assert.ok(address.startsWith(ADDRESS_PREFIX));
});

test("derive separates secrets, programs and seeds", () => {
  [
    derive(parseSecret("08".repeat(32)), "portal", SEED),
    derive(secret(), "local_prover", SEED),
    derive(secret(), "portal", "cd".repeat(32)),
  ].forEach(({ address }) => assert.notEqual(address, PORTAL_ADDRESS));
});

test("keypair bytes are the seed followed by its public key", () => {
  const { bytes } = derive(secret(), "portal", SEED);
  const privateKey = createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), bytes.subarray(0, 32)]),
    format: "der",
    type: "pkcs8",
  });
  const publicKey = createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);

  assert.equal(bytes.length, 64);
  assert.deepEqual(bytes.subarray(32), publicKey);
});

test("dependencies are direct released dependencies plus aggregator members", () => {
  assert.deepEqual(dependencies(metadata(), PROGRAMS), DEPENDENCIES);
});

test("dependencies follow unreleased workspace crates to the released programs behind them", () => {
  const throughShared = { ...CARGO_DEPENDENCIES, hyper_prover: ["eco-shared"], eco_shared: ["portal"] };

  assert.deepEqual(dependencies(metadata(throughShared), PROGRAMS).hyper_prover, ["portal"]);
});

test("dependencies reject unknown programs and an aggregator without its members", () => {
  assert.throws(() => dependencies(metadata(), [...PROGRAMS, "unknown"]), /not a workspace package/);
  assert.throws(
    () => dependencies(metadata(), ["aggregator_prover", "portal"]),
    /aggregator_prover is released without its members: hyper_prover, polymer_prover, layerzero_prover/,
  );
});

test("dependencies match the workspace", () => {
  const workspace = JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { cwd: REPOSITORY, encoding: "utf8" }),
  );

  assert.deepEqual(dependencies(workspace, PROGRAMS), DEPENDENCIES);
});

test("released programs match the Anchor cluster builds", () => {
  const workflow = readFileSync(join(REPOSITORY, ".github/workflows/release.yml"), "utf8");
  const released = workflow.match(/RELEASED_PROGRAMS: "([^"]+)"/)[1].split(" ").toSorted();
  const anchor = readFileSync(join(REPOSITORY, "Anchor.toml"), "utf8");

  ["build-mainnet", "build-devnet"].forEach((script) => {
    const command = anchor.match(new RegExp(`^${script} = "([^"]+)"`, "m"))[1];
    const built = [...command.matchAll(/--program-name ([a-z-]+)/g)]
      .map(([, name]) => name.replaceAll("-", "_"))
      .toSorted();

    assert.deepEqual(built, released, script);
  });
});

test("seeds are pinned", () => {
  const programSeed = programSeeds();

  assert.equal(programSeed.portal, PORTAL_SEED);
  assert.equal(programSeed.aggregator_prover, AGGREGATOR_SEED);
});

test("a seed moves with its own or a dependency's bytecode on either cluster", () => {
  assert.deepEqual(programSeeds(), programSeeds());
  assert.deepEqual(
    moved(programSeeds({ overrides: { portal: { devnet: Buffer.from("changed") } } })),
    PROGRAMS.filter((program) => program !== "proof_helper").toSorted(),
  );
  assert.deepEqual(moved(programSeeds({ overrides: { polymer_prover: { mainnet: Buffer.from("changed") } } })), [
    "aggregator_prover",
    "polymer_prover",
  ]);
  assert.deepEqual(moved(programSeeds({ overrides: { flash_fulfiller: { mainnet: Buffer.from("changed") } } })), [
    "flash_fulfiller",
    "local_prover",
  ]);
  assert.deepEqual(moved(programSeeds({ overrides: { layerzero_prover: { devnet: Buffer.from("changed") } } })), [
    "aggregator_prover",
    "layerzero_prover",
  ]);
  const swapped = { portal: { mainnet: Buffer.from("portal/devnet"), devnet: Buffer.from("portal/mainnet") } };
  assert.ok(moved(programSeeds({ overrides: swapped })).includes("portal"));
});

test("a dependency's move propagates even when no bytecode changes", () => {
  const extraEdge = { ...DEPENDENCIES, hyper_prover: ["flash_fulfiller", "portal"] };

  assert.deepEqual(moved(programSeeds({ programDependencies: extraEdge })), ["aggregator_prover", "hyper_prover"]);
});

test("a salt moves its program and everything depending on it", () => {
  assert.deepEqual(programSeeds({ salts: { portal: 0 } }), programSeeds());
  assert.deepEqual(
    moved(programSeeds({ salts: { portal: 1 } })),
    PROGRAMS.filter((program) => program !== "proof_helper").toSorted(),
  );
  assert.deepEqual(moved(programSeeds({ salts: { polymer_prover: 1 } })), ["aggregator_prover", "polymer_prover"]);
  [-1, 1.5, "1"].forEach((salt) =>
    assert.throws(() => programSeeds({ salts: { portal: salt } }), /must be a non-negative integer/),
  );
});

test("seeds require every placeholder build and reject cycles", () => {
  const incomplete = binaries();
  delete incomplete.portal.devnet;

  assert.throws(() => seeds(incomplete, DEPENDENCIES), /missing devnet placeholder build of portal/);
  assert.throws(
    () => programSeeds({ programDependencies: { ...DEPENDENCIES, portal: ["hyper_prover"] } }),
    /dependency cycle/,
  );
});

test("parsers reject malformed input", () => {
  assert.throws(() => parseSecret("zz".repeat(32)), /must be hex-encoded/);
  assert.throws(() => parseSecret("0".repeat(63)), /must be hex-encoded/);
  assert.throws(() => parseSecret("07".repeat(31)), /at least 32 bytes, got 31/);
  ["", "hyper-prover", "Portal", "portal "].forEach((program) =>
    assert.throws(() => parseProgram(program), /must be a crate lib name/),
  );
  ["", "AB".repeat(32), "ab".repeat(31)].forEach((seed) =>
    assert.throws(() => parseSeed(seed), /must be 64 lowercase hex/),
  );
});

test("release prints only the manifest that deploy turns into private keypair files", () => {
  const directory = temporary();
  ["mainnet", "devnet"].forEach((cluster) => {
    mkdirSync(join(directory, "placeholder", cluster), { recursive: true });
    writeFileSync(join(directory, "placeholder", cluster, "proof_helper.so"), `proof_helper/${cluster}`);
  });
  const released = run(["release", join(directory, "placeholder"), "proof_helper"], {
    PROGRAM_KEYPAIR_SECRET: SECRET_HEX,
  });
  assert.equal(released.status, 0, released.stderr);
  const manifest = JSON.parse(released.stdout);
  const { address, seed, salt } = manifest.proof_helper;
  assert.deepEqual(Object.keys(manifest), ["proof_helper"]);
  assert.equal(salt, 0);
  assert.equal(address, derive(secret(), "proof_helper", seed).address);

  const manifestPath = join(directory, "program-ids.json");
  const keys = join(directory, "keys");
  const file = join(keys, "proof_helper-keypair.json");
  writeFileSync(manifestPath, released.stdout);
  mkdirSync(keys);
  writeFileSync(file, "stale", { mode: 0o644 });
  const deployed = run(["deploy", manifestPath, keys], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX });

  assert.equal(deployed.status, 0, deployed.stderr);
  assert.equal(deployed.stdout, `proof_helper ${address}\n`);
  assert.equal(statSync(file).mode & 0o777, 0o600);
  assert.deepEqual(Buffer.from(JSON.parse(readFileSync(file, "utf8"))), derive(secret(), "proof_helper", seed).bytes);
  rmSync(directory, { recursive: true });
});

test("deploy rejects a manifest the secret does not reproduce", () => {
  const directory = temporary();
  const manifest = join(directory, "program-ids.json");
  writeFileSync(manifest, JSON.stringify({ portal: { address: PORTAL_ADDRESS, seed: SEED } }));
  const result = run(["deploy", manifest, join(directory, "keys")], { PROGRAM_KEYPAIR_SECRET: "08".repeat(32) });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /wrong secret/);
  assert.throws(() => statSync(join(directory, "keys")));
  rmSync(directory, { recursive: true });
});

test("cli reports a missing or blank secret and an unknown command", () => {
  [
    [run(["deploy", "program-ids.json", "keys"], {}), /must be set/],
    [run(["deploy", "program-ids.json", "keys"], { PROGRAM_KEYPAIR_SECRET: "" }), /must be set/],
    [run(["deploy", "program-ids.json", "keys"], { PROGRAM_KEYPAIR_SECRET: "  " }), /must be set/],
    [run(["derive"], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX }), /usage:/],
  ].forEach(([result, message]) => {
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, message);
  });
});

test("cli runs through a symlinked entry point", () => {
  const directory = temporary();
  const link = join(directory, "program-keypairs.mjs");
  symlinkSync(SCRIPT, link);
  const result = run(["derive"], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX }, { script: link });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /usage:/);
  rmSync(directory, { recursive: true });
});
