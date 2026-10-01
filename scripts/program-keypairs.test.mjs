import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createPrivateKey, createPublicKey } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
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
const SECRET_HEX = "07".repeat(32);
const SEED = "ab".repeat(32);
// Verified with `solana-keygen pubkey` on the written keypair file.
const PORTAL_ADDRESS = "Ecoi8woUrmkLq8PjpPFVF2k7xALmeKnAvWz2qVCkW1d7";
const PROGRAMS = [
  "aggregator_prover",
  "portal",
  "hyper_prover",
  "local_prover",
  "flash_fulfiller",
  "proof_helper",
  "polymer_prover",
];
const CARGO_DEPENDENCIES = {
  aggregator_prover: ["portal", "anchor-lang"],
  portal: ["eco-svm-std"],
  hyper_prover: ["portal"],
  local_prover: ["portal", "flash-fulfiller"],
  flash_fulfiller: ["portal"],
  proof_helper: [],
  polymer_prover: ["portal"],
};

const secret = () => parseSecret(SECRET_HEX);

const metadata = (cargoDependencies = CARGO_DEPENDENCIES) => ({
  packages: Object.entries(cargoDependencies).map(([program, names]) => ({
    name: program.replaceAll("_", "-"),
    dependencies: [
      ...names.map((name) => ({ name, kind: null })),
      { name: "portal", kind: "dev" },
    ],
  })),
});

const binaries = (overrides = {}) =>
  Object.fromEntries(
    PROGRAMS.map((program) => [
      program,
      { mainnet: Buffer.from(`${program}/mainnet`), devnet: Buffer.from(`${program}/devnet`), ...overrides[program] },
    ]),
  );

const programSeeds = (overrides) => seeds(binaries(overrides), dependencies(metadata(), PROGRAMS));

const run = (args, env) =>
  spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", env: { PATH: process.env.PATH, ...env } });

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

test("dependencies follow released Cargo dependencies transitively plus aggregator members", () => {
  assert.deepEqual(dependencies(metadata(), PROGRAMS), {
    aggregator_prover: ["flash_fulfiller", "hyper_prover", "local_prover", "polymer_prover", "portal"],
    portal: [],
    hyper_prover: ["portal"],
    local_prover: ["flash_fulfiller", "portal"],
    flash_fulfiller: ["portal"],
    proof_helper: [],
    polymer_prover: ["portal"],
  });
  assert.throws(() => dependencies(metadata({ portal: [] }), ["portal", "hyper_prover"]), /not a workspace package/);
});

test("a program's seed moves with its own or a dependency's bytecode, on either cluster", () => {
  const base = programSeeds();

  assert.deepEqual(programSeeds(), base);
  // portal changes: everything bound to portal moves, proof_helper keeps its address
  const portalChanged = programSeeds({ portal: { devnet: Buffer.from("changed") } });
  PROGRAMS.filter((program) => program !== "proof_helper").forEach((program) =>
    assert.notEqual(portalChanged[program], base[program], program),
  );
  assert.equal(portalChanged.proof_helper, base.proof_helper);
  // a leaf member changes: only it and the aggregator move
  const polymerChanged = programSeeds({ polymer_prover: { mainnet: Buffer.from("changed") } });
  PROGRAMS.forEach((program) =>
    ["polymer_prover", "aggregator_prover"].includes(program)
      ? assert.notEqual(polymerChanged[program], base[program], program)
      : assert.equal(polymerChanged[program], base[program], program),
  );
  // flash_fulfiller changes: local_prover compiles its ID in, the aggregator configures local_prover
  const flashChanged = programSeeds({ flash_fulfiller: { mainnet: Buffer.from("changed") } });
  assert.deepEqual(
    PROGRAMS.filter((program) => flashChanged[program] !== base[program]).toSorted(),
    ["aggregator_prover", "flash_fulfiller", "local_prover"],
  );
});

test("seeds require every placeholder build", () => {
  const incomplete = binaries();
  delete incomplete.portal.devnet;

  assert.throws(() => seeds(incomplete, dependencies(metadata(), PROGRAMS)), /missing devnet placeholder build of portal/);
});

test("parsers reject malformed input", () => {
  assert.throws(() => parseSecret("zz".repeat(32)), /must be hex-encoded/);
  assert.throws(() => parseSecret("0".repeat(63)), /must be hex-encoded/);
  assert.throws(() => parseSecret("07".repeat(31)), /at least 32 bytes, got 31/);
  ["", "hyper-prover", "Portal", "portal "].forEach((program) =>
    assert.throws(() => parseProgram(program), /must be a crate lib name/),
  );
  ["", "AB".repeat(32), "ab".repeat(31)].forEach((seed) => assert.throws(() => parseSeed(seed), /must be 64 lowercase hex/));
});

test("deploy writes private keypair files for a matching manifest", () => {
  const directory = mkdtempSync(join(tmpdir(), "program-keypairs-"));
  const manifest = join(directory, "program-ids.json");
  writeFileSync(manifest, JSON.stringify({ portal: { address: PORTAL_ADDRESS, seed: SEED } }));
  const result = run(["deploy", manifest, join(directory, "keys")], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX });
  const file = join(directory, "keys", "portal-keypair.json");

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, `portal ${PORTAL_ADDRESS}\n`);
  assert.equal(statSync(file).mode & 0o777, 0o600);
  assert.deepEqual(Buffer.from(JSON.parse(readFileSync(file, "utf8"))), derive(secret(), "portal", SEED).bytes);
  rmSync(directory, { recursive: true });
});

test("deploy rejects a manifest the secret does not reproduce", () => {
  const directory = mkdtempSync(join(tmpdir(), "program-keypairs-"));
  const manifest = join(directory, "program-ids.json");
  writeFileSync(manifest, JSON.stringify({ portal: { address: PORTAL_ADDRESS, seed: SEED } }));
  const result = run(["deploy", manifest, join(directory, "keys")], { PROGRAM_KEYPAIR_SECRET: "08".repeat(32) });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /wrong secret/);
  assert.throws(() => statSync(join(directory, "keys")));
  rmSync(directory, { recursive: true });
});

test("cli fails without a secret or a known command", () => {
  [run(["deploy", "program-ids.json", "keys"], {}), run(["derive"], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX })].forEach(
    (result) => {
      assert.equal(result.status, 1);
      assert.equal(result.stdout, "");
    },
  );
});
