import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const pin = (workflow, name) => {
  const text = readFileSync(new URL(`../.github/workflows/${workflow}`, import.meta.url), "utf8");
  const match = text.match(new RegExp(`^  ${name}: "([^"]+)"$`, "m"));
  assert.ok(match, `${workflow} pins ${name}`);

  return match[1];
};

for (const name of ["VERIFY_IMAGE", "SOLANA_VERIFY_VERSION"]) {
  test(`release and deploy workflows pin the same ${name}`, () => {
    assert.equal(pin("deploy.yml", name), pin("release.yml", name));
  });
}

test("deploy workflow runs steps with pipefail", () => {
  const text = readFileSync(new URL("../.github/workflows/deploy.yml", import.meta.url), "utf8");
  assert.match(text, /^defaults:\n {2}run:\n {4}shell: bash$/m);
  assert.doesNotMatch(text, /\|\| true|continue-on-error/);
});

test("deploy workflow tees only redacted output", () => {
  const text = readFileSync(new URL("../.github/workflows/deploy.yml", import.meta.url), "utf8");
  const tees = text.match(/^.*\| tee .*$/gm) ?? [];
  assert.ok(tees.length >= 5);
  tees.forEach((line) => assert.match(line, /deploy-step\.sh redact \| tee/));
});

test("redact strips the RPC URL and any URL-shaped token", () => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const rpcUrl = "https://rpc.example/?api-key=SECRET";
  const input = [
    `error sending request for url (${rpcUrl}): timeout`,
    "ws://other.example/v2/KEY2 and https://a.example/p?k=KEY3, done",
    "plain line",
  ].join("\n");
  const { stdout, status } = spawnSync("bash", [script, "redact"], {
    input,
    encoding: "utf8",
    env: { ...process.env, RPC_URL: rpcUrl },
  });
  assert.equal(status, 0);
  assert.doesNotMatch(stdout, /SECRET|KEY2|KEY3|example/);
  assert.match(stdout, /^plain line$/m);
});

test("deploy workflow rejects a malformed compute_unit_price before anything runs", () => {
  const text = readFileSync(new URL("../.github/workflows/deploy.yml", import.meta.url), "utf8");
  const steps = text.slice(text.indexOf("    steps:\n"));
  const first = steps.match(/^ {6}- name: (.+)\n(?: {8}.*\n| *\n)*?(?= {6}- name:)/m);
  assert.equal(first[1], "Validate inputs");
  const script = first[0]
    .slice(first[0].indexOf("        run: |\n") + "        run: |\n".length)
    .replace(/^ {10}/gm, "");
  const validates = (computeUnitPrice) =>
    spawnSync("bash", ["-c", script], {
      encoding: "utf8",
      env: { ...process.env, VERSION: "1.2.3", COMPUTE_UNIT_PRICE: computeUnitPrice },
    }).status === 0;

  ["0", "5", "9999999999999999999"].forEach((value) => assert.ok(validates(value), value));
  ["", "-1", "1.5", "abc", " 5", "5 ", "1e6", "99999999999999999999"].forEach((value) =>
    assert.ok(!validates(value), value),
  );
});
