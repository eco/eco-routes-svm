import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

const DEPLOY_WORKFLOWS = { "deploy.yml": 6, "finalize.yml": 2, "close.yml": 2 };
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

  test(`${name} plans without approval or keys, then writes only the approved plan`, () => {
    const text = workflow(name);
    const execute = text.indexOf("\n    needs: plan\n");
    const planJob = text.slice(text.indexOf("\n  plan:\n"), execute).replace(/^\s*#.*$/gm, "");
    const executeJob = text.slice(execute);
    assert.ok(execute > 0);
    assert.doesNotMatch(planJob, /environment:|DEPLOYER_KEYPAIR|PROGRAM_KEYPAIR_SECRET/);
    assert.match(planJob, /scripts\/deploy-step\.sh hash out\/plan\.log/);
    assert.equal(text.match(/^ {4}environment: release$/gm)?.length, 1);
    assert.match(executeJob, /^ {4}environment: release$/m);
    assert.match(executeJob, /PLAN_HASH: \$\{\{ needs\.plan\.outputs\.hash \}\}/);
    assert.match(executeJob, /scripts\/deploy-step\.sh gate out\/plan\.log/);
    assert.doesNotMatch(text, /plan_hash:|inputs\.plan_hash/);
    assert.match(text, /^concurrency:\n {2}group: deploy-\$\{\{ inputs\.cluster \}\}\n {2}cancel-in-progress: false$/m);
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
      env: { ...process.env, DEPLOYER: "Deployer", VERSION: "1.2.3", PROGRAMS: "all", COMPUTE_UNIT_PRICE: computeUnitPrice },
    }).status === 0;

  ["0", "5", "9999999999999999999"].forEach((value) => assert.ok(validates(value), value));
  ["", "-1", "1.5", "abc", " 5", "5 ", "1e6", "99999999999999999999"].forEach((value) =>
    assert.ok(!validates(value), value),
  );
  const unset = spawnSync("bash", ["-c", script], {
    encoding: "utf8",
    env: { ...process.env, DEPLOYER: "", VERSION: "1.2.3", PROGRAMS: "all", COMPUTE_UNIT_PRICE: "0" },
  });
  assert.notEqual(unset.status, 0);
  assert.match(unset.stderr, /DEPLOYER_ADDRESS/);
});

test("deploy workflow closes leftover buffers after any outcome, while the deployer key exists", () => {
  const text = readFileSync(new URL("../.github/workflows/deploy.yml", import.meta.url), "utf8");
  const close = text.indexOf("      - name: Close leftover buffers\n");
  const verify = text.indexOf("      - name: Verify\n");
  assert.ok(verify > 0 && close > verify);
  assert.ok(close < text.indexOf("      - name: Delete keys\n"));
  assert.match(text.slice(close), /^ {8}if: always\(\) && steps\.gate\.outcome == 'success'$/m);
});

test("deploy workflow uploads the IDLs after apply and before verify, while the deployer key exists", () => {
  const text = workflow("deploy.yml");
  const step = (name) => text.indexOf(`      - name: ${name}\n`);
  const upload = step("Upload IDLs");

  assert.ok(step("Apply") > 0 && upload > step("Apply"));
  assert.ok(upload < step("Verify") && upload < step("Delete keys"));
  assert.match(
    text.slice(upload, step("Verify")),
    /RPC_URL: https:\/\/solana-\$\{\{ inputs\.cluster \}\}\.g\.alchemy\.com\/v2\/\$\{\{ secrets\.ALCHEMY_API_KEY \}\}\n {8}run: scripts\/deploy-step\.sh idl 2>&1 \| scripts\/deploy-step\.sh redact \| tee out\/idl\.log\n/,
  );
  assert.doesNotMatch(workflow("finalize.yml"), /deploy-step\.sh idl/);
});

// Runs `deploy-step.sh idl` against a stub `deployer` exiting with `status`.
const idlStep = (status) => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const bin = mkdtempSync(join(tmpdir(), "deploy-step-"));
  const calls = join(bin, "calls");
  writeFileSync(join(bin, "deployer"), `#!/usr/bin/env bash\necho "deployer $*" >> "${calls}"\nexit ${status}\n`, {
    mode: 0o755,
  });
  const result = spawnSync("bash", [script, "idl"], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      RPC_URL: "http://rpc",
      KEYPAIR: "deployer.json",
      ASSETS: "release",
      PLAN: "out/plan.json",
    },
  });
  const sent = readFileSync(calls, "utf8");
  rmSync(bin, { recursive: true });

  return { ...result, sent };
};

test("idl uploads through the deployer, signed by the deployer keypair", () => {
  const { status, stderr, sent } = idlStep(0);

  assert.equal(status, 0, stderr);
  assert.equal(sent, "deployer idl --plan out/plan.json --assets release --deployer-keypair deployer.json\n");
});

test("a failed IDL upload fails the step and says how to resume", () => {
  const { status, stderr } = idlStep(1);

  assert.notEqual(status, 0);
  assert.match(stderr, /IDL upload failed; .*rerun the deploy workflow, whose IDL step resumes from the chain/);
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
  writeFileSync(join(bin, "plan.json"), JSON.stringify({
    inputs: { compute_unit_price: price },
    selected: ["portal"],
    release: { programs: { portal: {} } },
  }));

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

test("release check requires assets only for selected programs and preserves the full manifest", () => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const directory = mkdtempSync(join(tmpdir(), "selected-release-"));
  mkdirSync(join(directory, "programs", "intent-chainer", "src"), { recursive: true });
  writeFileSync(join(directory, "programs", "intent-chainer", "src", "lib.rs"), 'declare_id!("Chainer");');
  writeFileSync(join(directory, "intent_chainer.devnet.json"), '{"address":"Chainer"}');
  writeFileSync(join(directory, "intent_chainer.devnet.so"), "binary");
  const manifest = '{"intent_chainer":{"address":"Chainer"},"portal":{"address":"OldPortal"}}';
  writeFileSync(join(directory, "program-ids.json"), manifest);
  const result = spawnSync("bash", [script, "check"], {
    cwd: directory,
    encoding: "utf8",
    env: { ...process.env, ASSETS: directory, CLUSTER: "devnet", PROGRAMS: "intent_chainer" },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "ok intent_chainer Chainer\n");
  assert.equal(readFileSync(join(directory, "program-ids.json"), "utf8"), manifest);
  rmSync(directory, { recursive: true });
});

test("selected deploys use fresh explicit buffers and clean only those buffers after a failed upload", () => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const directory = mkdtempSync(join(tmpdir(), "selected-deploy-"));
  const calls = join(directory, "calls");
  const buffers = join(directory, "buffers");
  const stub = (name, body) =>
    writeFileSync(join(directory, name), `#!/usr/bin/env bash\n${body}\n`, { mode: 0o755 });
  stub("deployer", "echo intent_chainer");
  stub("solana-keygen", `[ "$1" = new ] || { echo FreshBuffer; exit; }
while [ "$1" != --outfile ]; do shift; done
printf 'buffer-key' > "$2"`);
  stub("solana", `echo "solana $*" >> "${calls}"
if [ "$2" = deploy ]; then
  [ "$(cat "${directory}/buffers.txt")" = "intent_chainer FreshBuffer" ] || exit 2
  exit 1
fi`);
  writeFileSync(join(directory, "program-ids.json"), '{"intent_chainer":{"address":"Chainer"},"portal":{"address":"OldPortal"}}');
  writeFileSync(join(directory, "deployment-policy.json"), '{"nondeployable_programs":["portal"]}');
  writeFileSync(join(directory, "plan.json"), '{"inputs":{"compute_unit_price":0},"selected":["intent_chainer"],"release":{"programs":{"intent_chainer":{},"portal":{}}}}');
  const env = {
    ...process.env,
    PATH: `${directory}:${process.env.PATH}`,
    ASSETS: directory,
    PLAN: join(directory, "plan.json"),
    KEYPAIR: "deployer.json",
    RPC_URL: "http://rpc",
    CLUSTER: "mainnet",
    KEYS_DIR: directory,
    BUFFER_DIR: buffers,
  };
  const deployed = spawnSync("bash", [script, "deploy"], { encoding: "utf8", env });
  assert.equal(deployed.status, 1, deployed.stderr);
  const cleaned = spawnSync("bash", [script, "close-buffers"], { encoding: "utf8", env });
  assert.equal(cleaned.status, 0, cleaned.stderr);
  const sent = readFileSync(calls, "utf8");
  assert.match(sent, new RegExp(`--buffer ${buffers}/intent_chainer-buffer-keypair.json`));
  assert.match(sent, /solana program close FreshBuffer -u http:\/\/rpc -k deployer.json --authority deployer.json/);
  assert.doesNotMatch(sent, /--buffers|OldPortal/);
  stub("solana", 'echo "RPC unavailable" >&2; exit 1');
  const failedCleanup = spawnSync("bash", [script, "close-buffers"], { encoding: "utf8", env });
  assert.equal(failedCleanup.status, 1);
  rmSync(buffers, { recursive: true });
  const record = readFileSync(join(directory, "buffers.txt"), "utf8");
  assert.equal(record, "intent_chainer FreshBuffer\n");
  assert.doesNotMatch(record, /buffer-key|OldPortal/);
  rmSync(directory, { recursive: true });
});

test("selected buffer cleanup skips consumed buffers but fails on RPC errors", () => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const directory = mkdtempSync(join(tmpdir(), "selected-buffers-"));
  const calls = join(directory, "calls");
  writeFileSync(join(directory, "intent_chainer-buffer-keypair.json"), "buffer-key");
  writeFileSync(join(directory, "deployment-policy.json"), '{"nondeployable_programs":["portal"]}');
  writeFileSync(join(directory, "solana-keygen"), "#!/usr/bin/env bash\necho FreshBuffer\n", { mode: 0o755 });
  const env = {
    ...process.env,
    PATH: `${directory}:${process.env.PATH}`,
    ASSETS: directory,
    BUFFER_DIR: directory,
    KEYPAIR: "deployer.json",
    RPC_URL: "http://rpc",
  };
  ["Unable to find the account FreshBuffer", "RPC unavailable"].forEach((message) => {
    writeFileSync(join(directory, "solana"),
      `#!/usr/bin/env bash\necho "$*" >> "${calls}"\necho "${message}" >&2\nexit 1\n`, { mode: 0o755 });
    const result = spawnSync("bash", [script, "close-buffers"], { encoding: "utf8", env });
    assert.equal(result.status, message.startsWith("Unable") ? 0 : 1, result.stderr);
  });
  assert.doesNotMatch(readFileSync(calls, "utf8"), /close/);
  rmSync(directory, { recursive: true });
});

const firstStepScript = (text) => {
  const steps = text.slice(text.indexOf("    steps:\n"));
  const first = steps.match(/^ {6}- name: (.+)\n(?: {8}.*\n| *\n)*?(?= {6}- name:)/m);
  assert.equal(first[1], "Validate inputs");

  return first[0].slice(first[0].indexOf("        run: |\n") + "        run: |\n".length).replace(/^ {10}/gm, "");
};

for (const name of ["deploy.yml", "finalize.yml", "close.yml"]) {
  test(`${name} rejects a malformed programs input before anything runs`, () => {
    const script = firstStepScript(workflow(name));
    const validates = (programs) =>
      spawnSync("bash", ["-c", script], {
        encoding: "utf8",
        env: { ...process.env, DEPLOYER: "Deployer", VERSION: "1.2.3", PROGRAMS: programs, COMPUTE_UNIT_PRICE: "0" },
      }).status === 0;

    ["all", "hyper_prover", "aggregator_prover,hyper_prover"].forEach((value) => assert.ok(validates(value), value));
    ["", "All", "hyper_prover,", ",hyper_prover", "hyper prover", "hyper_prover;rm", "all,hyper_prover"].forEach(
      (value) => assert.ok(!validates(value), value),
    );
  });
}

// Runs `deploy-step.sh <command>` on a plan log and returns its status, outputs and summary.
const planLogStep = (command, planHash, log) => {
  const directory = mkdtempSync(join(tmpdir(), "plan-log-"));
  const [planLog, output, summary] = ["plan.log", "output", "summary"].map((name) => join(directory, name));
  [output, summary].forEach((path) => writeFileSync(path, ""));
  writeFileSync(planLog, log);
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const { status, stderr } = spawnSync("bash", [script, command, planLog], {
    encoding: "utf8",
    env: { ...process.env, PLAN_HASH: planHash, GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary },
  });
  const result = { status, stderr, output: readFileSync(output, "utf8"), summary: readFileSync(summary, "utf8") };
  rmSync(directory, { recursive: true });

  return result;
};

test("hash hands the plan job's hash to the approved job", () => {
  const planned = planLogStep("hash", "", "planning\nplan_hash=abc\n");

  assert.equal(planned.status, 0);
  assert.equal(planned.output, "hash=abc\n");
  assert.match(planned.summary, /Approve the next job to execute exactly this plan \(`abc`\)/);
  assert.notEqual(planLogStep("hash", "", "no hash printed\n").status, 0);
});

test("gate lets the approved job write only the approved plan", () => {
  const log = "planning\nplan_hash=abc\n";

  assert.equal(planLogStep("gate", "abc", log).status, 0);
  const changed = planLogStep("gate", "abd", log);
  assert.notEqual(changed.status, 0);
  assert.match(changed.stderr, /changed since the plan was approved \(approved abd, now abc\)/);
  assert.notEqual(planLogStep("gate", "", log).status, 0);
  assert.notEqual(planLogStep("gate", "abc", "no hash printed\n").status, 0);
});

// Runs `deploy-step.sh close` against stub `solana` and `deployer` binaries; the stub `deployer`
// exits with `closeIdlStatus` on `close-idl`. Returns every call, in order.
const closeStep = (closeIdlStatus) => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const bin = mkdtempSync(join(tmpdir(), "deploy-step-"));
  const calls = join(bin, "calls");
  const stub = (name, body) => writeFileSync(join(bin, name), `#!/usr/bin/env bash\n${body}\n`, { mode: 0o755 });
  stub("solana", `echo "solana $*" >> "${calls}"`);
  stub("solana-keygen", "echo DeployerPubkey");
  stub(
    "deployer",
    `echo "deployer $*" >> "${calls}"
[ "$1" = close-idl ] && exit ${closeIdlStatus}
[ "$*" = "actions --plan plan.json --action close" ] && echo hyper_prover`,
  );
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
  const sent = readFileSync(calls, "utf8").replaceAll(bin, "ASSETS");
  rmSync(bin, { recursive: true });

  return { status, stderr, sent };
};

test("close closes each planned program's IDL accounts, then the program, returning the rent", () => {
  const { status, stderr, sent } = closeStep(0);

  assert.equal(status, 0, stderr);
  assert.equal(
    sent,
    "deployer close-idl --plan plan.json --assets ASSETS --deployer-keypair deployer.json\n" +
      "deployer actions --plan plan.json --action close\n" +
      "solana program close -u http://rpc -k deployer.json --authority deployer.json " +
      "--recipient DeployerPubkey --bypass-warning HyperAddress\n",
  );
});

test("close closes no program when its IDL accounts could not be closed", () => {
  const { status, sent } = closeStep(1);

  assert.notEqual(status, 0);
  assert.doesNotMatch(sent, /solana program close/);
});

test("verify fails when solana-verify reports a hash mismatch, which it exits 0 on", () => {
  const script = fileURLToPath(new URL("./deploy-step.sh", import.meta.url));
  const verify = (report) => {
    const bin = mkdtempSync(join(tmpdir(), "deploy-step-"));
    const stub = (name, body) => writeFileSync(join(bin, name), `#!/usr/bin/env bash\n${body}\n`, { mode: 0o755 });
    stub("solana-verify", `echo "${report}"`);
    stub("solana-keygen", "echo DeployerPubkey");
    stub("deployer", '[ "$*" = "actions --plan plan.json --action verify" ] && echo hyper_prover');
    writeFileSync(join(bin, "program-ids.json"), '{"hyper_prover":{"address":"HyperAddress"}}');
    const result = spawnSync("bash", [script, "verify"], {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${bin}:${process.env.PATH}`,
        RPC_URL: "http://rpc",
        KEYPAIR: "deployer.json",
        CLUSTER: "devnet",
        VERIFY_IMAGE: "image",
        ASSETS: bin,
        PLAN: "plan.json",
      },
    });
    rmSync(bin, { recursive: true });

    return result;
  };

  const matched = verify("Program hash matches ✅");
  const mismatched = verify("Program hashes do not match ❌");

  assert.equal(matched.status, 0, matched.stderr);
  assert.notEqual(mismatched.status, 0);
  assert.match(mismatched.stderr, /hyper_prover: the build from \w+ does not match the deployed bytecode/);
});

test("finalize.yml and close.yml differ only in their operation", () => {
  const steps = (name, operation, title) =>
    workflow(name)
      .replace(/^\s*#.*\n|^\s*description: .*\n/gm, "")
      .replaceAll(operation, "<operation>")
      .replaceAll(title, "<Operation>");

  assert.equal(steps("close.yml", "close", "Close"), steps("finalize.yml", "finalize", "Finalize"));
});
