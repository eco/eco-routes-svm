import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

const DEPLOY_WORKFLOWS = { "deploy.yml": 5, "finalize.yml": 2, "close.yml": 2 };
const workflow = (name) => readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), "utf8");

for (const [name, minimumTees] of Object.entries(DEPLOY_WORKFLOWS)) {
  test(`${name} runs steps with pipefail`, () => {
    const text = workflow(name);
    assert.match(text, /^defaults:\n {2}run:\n {4}shell: bash$/m);
    assert.doesNotMatch(text, /\|\| true|continue-on-error/);
  });

  test(`${name} tees only redacted output`, () => {
    const tees = workflow(name).match(/^.*\| tee .*$/gm) ?? [];
    assert.ok(tees.length >= minimumTees);
    tees.forEach((line) => assert.match(line, /deploy-step\.sh redact \| tee/));
  });

  test(`${name} needs release approval and never overlaps another run on its cluster`, () => {
    const text = workflow(name);
    assert.match(text, /^ {4}environment: release$/m);
    assert.match(text, /^concurrency:\n {2}group: deploy-\$\{\{ inputs\.cluster \}\}\n {2}cancel-in-progress: false$/m);
    assert.match(text, /scripts\/deploy-step\.sh gate out\/plan\.log/);
  });
}

test("deploy never finalizes or closes; each of the other workflows does only its own", () => {
  assert.doesNotMatch(workflow("deploy.yml"), /deploy-step\.sh (finalize|close)(?![-\w])/);
  assert.doesNotMatch(workflow("finalize.yml"), /deploy-step\.sh (deploy|close)\b|deployer apply/);
  assert.doesNotMatch(workflow("close.yml"), /deploy-step\.sh (deploy|finalize)\b|deployer apply/);
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

test("deploy workflow closes leftover buffers after any outcome, while the deployer key exists", () => {
  const text = readFileSync(new URL("../.github/workflows/deploy.yml", import.meta.url), "utf8");
  const close = text.indexOf("      - name: Close leftover buffers\n");
  assert.ok(close > text.indexOf("      - name: Finalize\n"));
  assert.ok(close < text.indexOf("      - name: Delete keys\n"));
  assert.match(
    text.slice(close),
    /^ {8}if: always\(\) && steps\.plan\.outputs\.execute == 'true'$/m,
  );
});

// Runs `deploy-step.sh deploy` against stub `solana` and `deployer` commands and returns the
// `solana` calls it made.
const deployCalls = (price) => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const bin = mkdtempSync(join(tmpdir(), "deploy-step-"));
  const calls = join(bin, "calls");
  writeFileSync(join(bin, "solana"), `#!/usr/bin/env bash\necho "solana $*" >> "${calls}"\n`, {
    mode: 0o755,
  });
  writeFileSync(join(bin, "deployer"), `#!/usr/bin/env bash\necho portal\n`, { mode: 0o755 });
  writeFileSync(join(bin, "program-ids.json"), '{"portal":{"address":"PortalAddress"}}');
  writeFileSync(join(bin, "plan.json"), JSON.stringify({ inputs: { compute_unit_price: price } }));

  const { status, stderr } = spawnSync("bash", [script, "deploy"], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      RPC_URL: "http://rpc",
      KEYPAIR: "deployer.json",
      ASSETS: bin,
      CLUSTER: "devnet",
      PLAN: join(bin, "plan.json"),
      COMPUTE_UNIT_PRICE: "999",
    },
  });
  assert.equal(status, 0, stderr);
  const sent = readFileSync(calls, "utf8").trim().split("\n");
  rmSync(bin, { recursive: true });

  return { sent, binary: `${bin}/portal.devnet.so` };
};

test("deploy closes the deployer's buffers before deploying anything", () => {
  const { sent, binary } = deployCalls("0");

  assert.deepEqual(sent, [
    "solana program close --buffers -u http://rpc -k deployer.json",
    "solana program deploy -u http://rpc -k deployer.json --upgrade-authority deployer.json " +
      `--program-id target/keys/portal-keypair.json --use-rpc --max-sign-attempts 20 ${binary}`,
  ]);
});

test("deploy takes the compute-unit price from the reviewed plan, not the environment", () => {
  const { sent, binary } = deployCalls("7");

  assert.equal(
    sent[1],
    "solana program deploy -u http://rpc -k deployer.json --upgrade-authority deployer.json " +
      "--program-id target/keys/portal-keypair.json --use-rpc --max-sign-attempts 20 " +
      `--with-compute-unit-price 7 ${binary}`,
  );
});

const firstStepScript = (text) => {
  const steps = text.slice(text.indexOf("    steps:\n"));
  const first = steps.match(/^ {6}- name: (.+)\n(?: {8}.*\n| *\n)*?(?= {6}- name:)/m);
  assert.equal(first[1], "Validate inputs");

  return first[0].slice(first[0].indexOf("        run: |\n") + "        run: |\n".length).replace(/^ {10}/gm, "");
};

for (const name of ["finalize.yml", "close.yml"]) {
  test(`${name} rejects a malformed programs input before anything runs`, () => {
    const script = firstStepScript(workflow(name));
    const validates = (programs) =>
      spawnSync("bash", ["-c", script], {
        encoding: "utf8",
        env: { ...process.env, VERSION: "1.2.3", PROGRAMS: programs },
      }).status === 0;

    ["all", "hyper_prover", "aggregator_prover,hyper_prover"].forEach((value) => assert.ok(validates(value), value));
    ["", "All", "hyper_prover,", ",hyper_prover", "hyper prover", "hyper_prover;rm", "all,hyper_prover"].forEach(
      (value) => assert.ok(!validates(value), value),
    );
  });
}

// Runs `deploy-step.sh gate` on a plan log and returns its outputs and summary.
const gate = (planHash, log) => {
  const directory = mkdtempSync(join(tmpdir(), "gate-"));
  const files = ["plan.log", "output", "summary"].map((name) => join(directory, name));
  const [planLog, output, summary] = files;
  [output, summary].forEach((path) => writeFileSync(path, ""));
  writeFileSync(planLog, log);
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const { status } = spawnSync("bash", [script, "gate", planLog], {
    encoding: "utf8",
    env: { ...process.env, PLAN_HASH: planHash, GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary },
  });
  const result = { status, output: readFileSync(output, "utf8"), summary: readFileSync(summary, "utf8") };
  rmSync(directory, { recursive: true });

  return result;
};

test("gate executes only when the reviewed hash equals the computed one", () => {
  const log = "planning\nplan_hash=abc\n";

  assert.equal(gate("abc", log).output, "computed=abc\nexecute=true\n");
  assert.equal(gate("", log).output, "computed=abc\nexecute=false\n");
  assert.match(gate("", log).summary, /Dry run \(no plan_hash given\).*plan_hash=`abc`/);
  assert.match(gate("abd", log).summary, /Dry run \(plan_hash differs/);
  const missing = gate("abc", "no hash printed\n");
  assert.notEqual(missing.status, 0);
  assert.equal(missing.output, "");
});

test("close closes each planned program and returns its rent to the deployer", () => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const bin = mkdtempSync(join(tmpdir(), "deploy-step-"));
  const calls = join(bin, "calls");
  const stub = (name, body) => writeFileSync(join(bin, name), `#!/usr/bin/env bash\n${body}\n`, { mode: 0o755 });
  stub("solana", `echo "solana $*" >> "${calls}"`);
  stub("solana-keygen", "echo DeployerPubkey");
  stub("deployer", '[ "$*" = "actions --plan plan.json --action close" ] && echo hyper_prover');
  writeFileSync(join(bin, "program-ids.json"), '{"hyper_prover":{"address":"HyperAddress"}}');

  const { status, stderr } = spawnSync("bash", [script, "close"], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      RPC_URL: "http://rpc",
      KEYPAIR: "deployer.json",
      ASSETS: bin,
      PLAN: "plan.json",
    },
  });

  assert.equal(status, 0, stderr);
  assert.equal(
    readFileSync(calls, "utf8"),
    "solana program close -u http://rpc -k deployer.json --authority deployer.json " +
      "--recipient DeployerPubkey --bypass-warning HyperAddress\n",
  );
  rmSync(bin, { recursive: true });
});
