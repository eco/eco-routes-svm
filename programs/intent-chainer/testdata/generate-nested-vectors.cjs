// Independent expectations, verified against the EXACT pinned Solidity harness.
// Run with NODE_PATH=<eco-routes>/node_modules, a fresh `forge build
// test/chain/IntentTemplate.t.sol` in that checkout, and anvil on localhost:18545.
// Only prints JSON to stdout. Never connects to a live chain or writes source files.
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const assert = require('node:assert/strict');
const e = require('ethers');
const { PublicKey } = require('@solana/web3.js');
const { TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID } = require('@solana/spl-token');
const ref = 'f8572a1971a7302d719878281bd3556fcb183463';
const repo = path.resolve(process.argv[2] || '../eco-routes');
assert.equal(execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repo, encoding: 'utf8' }).trim(), ref);
assert.equal(execFileSync('git', ['diff', '--name-only', 'HEAD', '--', 'contracts', 'test/chain'], { cwd: repo, encoding: 'utf8' }).trim(), '');
const artifact = (file, name) => JSON.parse(fs.readFileSync(path.join(repo, 'out', file + '.sol', name + '.json')));
const abi = e.AbiCoder.defaultAbiCoder();
const wad = 10n ** 18n;
const word = (n) => e.zeroPadValue(e.toBeHex(n), 32);
const addr = (n) => e.zeroPadValue(e.toBeHex(n), 20);
const ceil = (a, s) => (a * s + wad - 1n) / wad;
const hash = (destination, route, reward) => e.solidityPackedKeccak256(['uint64', 'bytes32', 'bytes32'], [destination, e.keccak256(route), e.keccak256(reward)]);
const amount = (source = 'Output', scale = wad, width = 32, little_endian = false) => ({ amount: { source, scale: String(scale), width, little_endian } });
const literal = (bytes) => ({ segments: [bytes], items: [] });
const one = (item) => ({ segments: ['0x', '0x'], items: [item] });
const render = (t, input, output, recipients) => e.concat(t.segments.flatMap((s, i) => {
  if (i === t.items.length) return [s];
  const item = t.items[i];
  if ('vault' in item) return [s, recipients[item.vault]];
  const a = item.amount;
  const value = ceil(a.source === 'Input' ? input : output, BigInt(a.scale));
  let encoded = e.getBytes(e.zeroPadValue(e.toBeHex(value), a.width));
  if (a.little_endian) encoded = encoded.reverse();
  return [s, encoded];
}));
const remoteRecipient = (config, h) => {
  if ('evm' in config) {
    const c = config.evm;
    if (c.prefix === 255) return e.zeroPadValue(e.getCreate2Address(c.portal, h, c.init_code_hash), 32).toLowerCase();
    return '0x' + '00'.repeat(12) + e.keccak256(e.concat([e.toBeHex(c.prefix, 1), c.portal, h, c.init_code_hash])).slice(-40);
  }
  const c = config.solana;
  const key = (hex) => new PublicKey(e.getBytes(hex));
  const [vault] = PublicKey.findProgramAddressSync([Buffer.from('vault'), e.getBytes(h)], key(c.portal));
  const [ata] = PublicKey.findProgramAddressSync([vault.toBuffer(), key(c.token_program).toBuffer(), key(c.mint).toBuffer()], ASSOCIATED_TOKEN_PROGRAM_ID);
  return e.hexlify(ata.toBuffer());
};
const toEvmTemplate = (t) => ({ segments: t.segments, items: t.items.map((i) => 'vault' in i
  ? { kind: 2, config: abi.encode(['uint256'], [i.vault]) }
  : { kind: 1, config: abi.encode(['uint8', 'uint256', 'uint8', 'bool'], [i.amount.source === 'Input' ? 1 : 2, i.amount.scale, i.amount.width, i.amount.little_endian]) }) });
const toEvmProgram = (p) => ({ route: toEvmTemplate(p.route), vaults: p.vaults.map((v) => {
  const c = v.derivation.evm || v.derivation.solana;
  return { destination: v.destination, route: toEvmTemplate(v.route), reward: toEvmTemplate(v.reward), derivation: v.derivation.evm
    ? { kind: 1, config: abi.encode(['address', 'bytes1', 'address', 'bytes32'], [c.portal, e.toBeHex(c.prefix, 1), c.implementation, c.init_code_hash]) }
    : { kind: 2, config: abi.encode(['bytes32', 'bytes32', 'bytes32'], [c.portal, c.token_program, c.mint]) } };
}) });
// Full abi.encode(Reward), INCLUDING its initial tuple offset.
const rewardType = 'tuple(uint64 deadline,address creator,address prover,uint256 nativeAmount,tuple(address token,uint256 amount)[] tokens)';
const reward = (value) => abi.encode([rewardType], [[2000000000, addr(0x123), addr(0x456), 0, [[addr(0x789), value]]]]);
const rewardTemplate = { segments: [reward(0).slice(0, -64), '0x'], items: [amount()] };
const routeType = 'tuple(bytes32 salt,uint64 deadline,address portal,uint256 nativeAmount,tuple(address token,uint256 amount)[] tokens,tuple(address target,bytes data,uint256 value)[] calls)';
const messenger = addr(0xcc); // deterministic fixture address, not a deployment.
const erc20 = new e.Interface(['function approve(address,uint256)']);
const cctp = new e.Interface(['function depositForBurn(uint256,uint32,bytes32,address)']);
const bridgeRoute = (value, recipient) => abi.encode([routeType], [[word(11), 2000000000, addr(0xabc), 0, [[addr(0x789), value]], [
  [addr(0x789), erc20.encodeFunctionData('approve', [messenger, value]), 0],
  [messenger, cctp.encodeFunctionData('depositForBurn', [value, 5, recipient, addr(0x789)]), 0],
]]]);
function cutBridge() {
  const am = 'a7'.repeat(32), rec = 'b8'.repeat(32);
  const blob = bridgeRoute(BigInt('0x' + am), '0x' + rec).slice(2);
  const segments = [], items = [];
  let cursor = 0;
  for (const match of blob.matchAll(new RegExp(am + '|' + rec, 'g'))) {
    segments.push('0x' + blob.slice(cursor, match.index));
    items.push(match[0] === am ? amount() : { vault: 0 });
    cursor = match.index + 64;
  }
  segments.push('0x' + blob.slice(cursor));
  assert.equal(items.length, 4);
  return { segments, items };
}
async function main() {
  const provider = new e.JsonRpcProvider('http://127.0.0.1:18545');
  const signer = await provider.getSigner(0);
  const deploy = async (file, name = file) => {
    const a = artifact(file, name);
    const contract = await new e.ContractFactory(a.abi, a.bytecode.object, signer).deploy();
    await contract.waitForDeployment(); return contract;
  };
  const harness = await deploy('IntentTemplate.t', 'TemplateHarness');
  const vault = await deploy('Vault'), tron = await deploy('VaultTron');
  const proxyCode = artifact('Proxy', 'Proxy').bytecode.object;
  const init = (implementation) => e.keccak256(e.concat([proxyCode, abi.encode(['address'], [implementation])]));
  const standard = { evm: { portal: '0xec000064576f9c95a8623bc0eff3db6d296ea6df', prefix: 255, implementation: await vault.getAddress(), init_code_hash: init(await vault.getAddress()) } };
  const world = { evm: { ...standard.evm, portal: '0x95b1eb197b1f9c9035450a7d12722e397de8f9eb' } };
  const tronConfig = { evm: { ...standard.evm, prefix: 65, implementation: await tron.getAddress(), init_code_hash: init(await tron.getAddress()) } };
  const solana = { solana: {
    portal: e.hexlify(new PublicKey('EcooswwC1NggsckZyF5SeAL9WsgJs3UhPbrqY1apV73F').toBuffer()),
    token_program: e.hexlify(TOKEN_PROGRAM_ID.toBuffer()),
    mint: e.hexlify(new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v').toBuffer()),
  } };
  const cases = [];
  const remoteRoute = () => {
    const marker = 'a7'.repeat(32);
    const blob = abi.encode([routeType], [[word(91), 2000000000, addr(0xabc), 0, [[addr(0x789), BigInt('0x' + marker)]], []]]).slice(2);
    const segments = blob.split(marker).map((s) => '0x' + s);
    assert.equal(segments.length, 2);
    return { segments, items: [amount()] };
  };
  for (const [name, derivation] of Object.entries({ standard, world, tron: tronConfig, solana })) {
    const program = { vaults: [{ destination: 480, route: remoteRoute(), reward: rewardTemplate, derivation }], route: cutBridge() };
    // Solana vault with actual Portal Borsh Reward; EVM variants keep full ABI.
    if (name === 'solana') {
      const prefix = e.concat(['0x0094357700000000', word(3), word(7), '0x0000000000000000', '0x01000000', solana.solana.mint]);
      program.vaults[0].reward = { segments: [prefix, '0x'], items: [amount('Output', wad, 8, true)] };
    }
    for (const input of [1000n, 1001n]) {
      const scale = wad * 96n / 100n, output = ceil(input, scale);
      const v = program.vaults[0], r = render(v.route, input, output, []), w = render(v.reward, input, output, []);
      const h = hash(v.destination, r, w), recipient = remoteRecipient(derivation, h);
      const expected = bridgeRoute(output, recipient);
      assert.equal(render(program.route, input, output, [recipient]), expected);
      assert.equal(await harness.render(toEvmProgram(program), input, output), expected);
      cases.push({ name: name + input, input: String(input), scale: String(scale), output: String(output), program, nodes: [{ route: r, reward: w, hash: h, recipient }], expected_route: expected });
    }
  }
  // Three dependency levels, a shared node in route AND reward, mixed VMs, and
  // per-item Input scale differing from the initial Output. Never remeasure.
  const program = { vaults: [
    { destination: 8453, route: remoteRoute(), reward: rewardTemplate, derivation: world },
    { destination: 1399811150, route: one({ vault: 0 }), reward: { segments: ['0x12', '0x34', '0x'], items: [{ vault: 0 }, amount('Input', wad / 2n, 8, true)] }, derivation: solana },
    { destination: 1, route: one({ vault: 1 }), reward: one({ vault: 0 }), derivation: tronConfig },
  ], route: { segments: ['0xaa', '0xbb', '0xcc', '0xdd', '0xee'], items: [{ vault: 2 }, { vault: 1 }, { vault: 0 }, amount('Input', wad / 2n, 8, true)] } };
  const input = 515n, scale = 2n * wad, output = ceil(input, scale), nodes = [], recipients = [];
  for (const v of program.vaults) {
    const r = render(v.route, input, output, recipients), w = render(v.reward, input, output, recipients);
    const h = hash(v.destination, r, w), recipient = remoteRecipient(v.derivation, h);
    nodes.push({ route: r, reward: w, hash: h, recipient }); recipients.push(recipient);
  }
  const expected = render(program.route, input, output, recipients);
  assert.equal(await harness.render(toEvmProgram(program), input, output), expected);
  cases.push({ name: 'three_levels_shared', input: String(input), scale: String(scale), output: String(output), program, nodes, expected_route: expected });
  const numeric = [];
  const max = (1n << 128n) - 1n;
  for (const a of [0n, 1n, 3n, (1n << 64n) - 1n, 10n ** 24n, max / 2n, max]) {
    for (const s of [1n, wad / 2n, wad - 1n, wad, wad + 1n, 10n ** 30n, max]) {
      const result = ceil(a, s);
      numeric.push({ amount: String(a), scale: String(s), expected: result <= max ? String(result) : null });
    }
  }
  console.log(JSON.stringify({ provenance: { ref, generator: 'ethers ABI/CREATE2 + Solana SDK; all route outputs checked by pinned TemplateHarness on local Anvil', proxy_creation_code: proxyCode }, cases, numeric }, null, 2));
  await provider.destroy();
}
main().catch((err) => { console.error(err); process.exitCode = 1; });
