import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createPrivateKey, createPublicKey } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { ADDRESS_PREFIX, derive, parseProgram, parseSecret, parseVersion } from "./program-keypairs.mjs";

const SCRIPT = fileURLToPath(new URL("./program-keypairs.mjs", import.meta.url));
const SECRET_HEX = "07".repeat(32);
// Cross-checked against an independent ed25519-dalek implementation of the same derivation.
const EXPECTED = {
  aggregator_prover: "EcogeoKjAAUWeSKYdye3mVqRomDid4qErdiJuCpx7fBV",
  portal: "Ecoum6DVDXfpV5ebqZEwpbKqRcTJN9cePm2ZVSyqDn25",
  hyper_prover: "EcoWB6H45iYQbRyjci8DDQvi5MYc2QdF1vsp9sjvtTnA",
};

const secret = () => parseSecret(SECRET_HEX);

const run = (args, env) =>
  spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", env: { PATH: process.env.PATH, ...env } });

test("derive is deterministic and Eco-prefixed", () => {
  Object.entries(EXPECTED).forEach(([program, address]) => {
    const keypair = derive(secret(), "1.2.3", program);

    assert.equal(keypair.address, address);
    assert.ok(keypair.address.startsWith(ADDRESS_PREFIX));
  });
});

test("derive separates secrets, versions and programs", () => {
  const base = derive(secret(), "1.2.3", "portal").address;

  [
    derive(parseSecret("08".repeat(32)), "1.2.3", "portal"),
    derive(secret(), "1.2.4", "portal"),
    derive(secret(), "1.2.3", "local_prover"),
  ].forEach(({ address }) => assert.notEqual(address, base));
});

test("keypair bytes are the seed followed by its public key", () => {
  const { bytes } = derive(secret(), "1.2.3", "portal");
  const privateKey = createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), bytes.subarray(0, 32)]),
    format: "der",
    type: "pkcs8",
  });
  const publicKey = createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);

  assert.equal(bytes.length, 64);
  assert.deepEqual(bytes.subarray(32), publicKey);
});

test("parseSecret rejects non-hex and short secrets", () => {
  assert.throws(() => parseSecret("zz".repeat(32)), /must be hex-encoded/);
  assert.throws(() => parseSecret("0".repeat(63)), /must be hex-encoded/);
  assert.throws(() => parseSecret("07".repeat(31)), /at least 32 bytes, got 31/);
});

test("parseVersion rejects non-semver versions", () => {
  ["v1.2.3", "1.2", "01.2.3", ""].forEach((version) =>
    assert.throws(() => parseVersion(version), /must be semver/),
  );
  assert.equal(parseVersion("1.2.3-rc.1+build.5"), "1.2.3-rc.1+build.5");
});

test("parseProgram rejects non-lib names", () => {
  ["", "hyper-prover", "Portal", "portal "].forEach((program) =>
    assert.throws(() => parseProgram(program), /must be a crate lib name/),
  );
});

test("cli writes private keypair files and prints addresses", () => {
  const outDir = join(mkdtempSync(join(tmpdir(), "program-keypairs-")), "keys");
  const result = run(["1.2.3", outDir, "portal", "hyper_prover"], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, `portal ${EXPECTED.portal}\nhyper_prover ${EXPECTED.hyper_prover}\n`);
  ["portal", "hyper_prover"].forEach((program) => {
    const file = join(outDir, `${program}-keypair.json`);
    const bytes = Buffer.from(JSON.parse(readFileSync(file, "utf8")));

    assert.equal(statSync(file).mode & 0o777, 0o600);
    assert.deepEqual(bytes, derive(secret(), "1.2.3", program).bytes);
  });
  rmSync(outDir, { recursive: true });
});

test("cli fails without a secret or programs", () => {
  [
    run(["1.2.3", "keys", "portal"], {}),
    run(["1.2.3", "keys"], { PROGRAM_KEYPAIR_SECRET: SECRET_HEX }),
  ].forEach((result) => {
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
  });
});
