/* tests/evm-to-svm.e2e.ts
 *
 * Scenario EVM → SVM → EVM
 *   1. publish&fund intent on EVM (Hardhat chain)
 *   2. fulfill on Solana dev/test-net
 *   3. prove + withdraw on EVM
 */

import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  VersionedTransaction,
  TransactionMessage,
} from "@solana/web3.js";
import {
  AnchorProvider,
  BN,
  Program,
  utils as anchorUtils,
} from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import { TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { expect } from "chai";
import { ethers, Signer } from "ethers";
import { ethers as hardhatEthers } from "hardhat";
import {
  TestERC20__factory,
  IntentSource__factory,
  TestProver__factory,
  IntentSource,
  TestERC20,
  TestProver,
} from "./evm-types";
import { EcoRoutes } from "../../target/types/eco_routes";
import {
  TESTNET_RPC,
  MAILBOX_ID_TESTNET,
  SPL_NOOP_ID,
  SOLANA_DOMAIN_ID,
  EVM_DOMAIN_ID,
} from "./constants";
import {
  evmUsdcAmount,
  encodeRoute,
  encodeReward,
  hashIntent,
  Route,
  Reward,
  TokenAmount,
} from "./evmUtils";
import { loadKeypairFromFile, usdcAmount } from "./utils";

anchor.setProvider(
  new AnchorProvider(
    new Connection(TESTNET_RPC, "confirmed"),
    new anchor.Wallet(Keypair.generate()),
    { commitment: "confirmed" }
  )
);

const provider = anchor.getProvider() as AnchorProvider;
const connection = provider.connection;
const program = anchor.workspace.EcoRoutes as Program<EcoRoutes>;

const feePayer = loadKeypairFromFile("../../keys/program_auth.json");
const solver = Keypair.generate(); // SVM solver key
const destinationUser = Keypair.generate(); // SVM recipient key

// fill once during EVM publish
let intentHashBytes!: Uint8Array;
let route!: Route;
let reward!: Reward;

// Re-use same 32-byte salt cross-chains
const salt = anchorUtils.bytes.utf8.encode("evm-svm-e2e".padEnd(32, "\0"));

describe("EVM → SVM e2e", () => {
  let usdc: TestERC20;
  let intentSource: IntentSource;
  let testProver: TestProver;
  let creator!: Signer;
  let solverEvm!: Signer;
  let intentHashHex!: string;

  it("publishes & funds on EVM", async () => {
    [creator, solverEvm] =
      (await hardhatEthers.getSigners()) as unknown as Signer[];

    // deploy mock USDC
    usdc = await new TestERC20__factory(creator).deploy("USDC", "USDC", 6);
    await usdc.waitForDeployment();

    // deploy IntentSource
    intentSource = await new IntentSource__factory(creator).deploy();
    await intentSource.waitForDeployment();

    // deploy lightweight TestProver and give its addr to reward.prover
    testProver = await new TestProver__factory(creator).deploy(
      await intentSource.getAddress()
    );
    await testProver.waitForDeployment();

    const creatorAddress = await creator.getAddress();
    const intentSourceAddress = await intentSource.getAddress();
    const usdcAddress = await usdc.getAddress();
    const testProverAddress = await testProver.getAddress();

    await usdc.mint(creatorAddress, evmUsdcAmount(1000));

    // build Route + Reward structs
    const saltHex = "0x" + Buffer.from(salt).toString("hex");

    const routeTokens: TokenAmount[] = [
      { token: usdcAddress, amount: evmUsdcAmount(5) },
    ];

    const rewardTokens: TokenAmount[] = [
      { token: usdcAddress, amount: evmUsdcAmount(1) },
    ];

    route = {
      salt: saltHex,
      source: EVM_DOMAIN_ID,
      destination: SOLANA_DOMAIN_ID,
      inbox: ethers.ZeroAddress, // will be validated on Solana side
      tokens: routeTokens,
      calls: [],
    };

    reward = {
      creator: creatorAddress,
      prover: testProverAddress,
      deadline: BigInt(0),
      nativeValue: BigInt(0),
      tokens: rewardTokens,
    };

    // allow IntentSource to pull USDC + publishAndFund
    await usdc.approve(intentSourceAddress, evmUsdcAmount(6));
    await intentSource.publishAndFund({ route, reward }, false);

    // produce 32-byte intent hash for Solana flow
    intentHashHex = hashIntent(encodeRoute(route), encodeReward(reward));
    intentHashBytes = Uint8Array.from(
      Buffer.from(intentHashHex.slice(2), "hex")
    );

    expect(intentHashBytes.length).equals(32);
  });

  it("fulfills intent on Solana", async () => {
    const executionAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("execution_authority"), salt],
      program.programId
    )[0];

    const dispatchAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("dispatch_authority")],
      program.programId
    )[0];

    const intentFulfillmentMarker = PublicKey.findProgramAddressSync(
      [Buffer.from("intent_fulfillment_marker"), intentHashBytes],
      program.programId
    )[0];

    const outboxPda = PublicKey.findProgramAddressSync(
      [Buffer.from("hyperlane"), Buffer.from("-"), Buffer.from("outbox")],
      MAILBOX_ID_TESTNET
    )[0];

    const dispatchedMessagePda = PublicKey.findProgramAddressSync(
      [
        Buffer.from("hyperlane"),
        Buffer.from("-"),
        Buffer.from("dispatched_message"),
        Buffer.from("-"),
        Keypair.generate().publicKey.toBuffer(),
      ],
      MAILBOX_ID_TESTNET
    )[0];

    const fulfillIx = await program.methods
      .fulfillIntent({
        intentHash: Array.from(intentHashBytes),
        route: {
          salt: Array.from(salt),
          sourceDomainId: EVM_DOMAIN_ID,
          destinationDomainId: SOLANA_DOMAIN_ID,
          inbox: Array(32).fill(7), // dummy inbox – not used here
          tokens: [
            {
              token: Array.from(new PublicKey(ethers.ZeroAddress).toBytes()),
              amount: new BN(usdcAmount(5)),
            },
          ],
          calls: [],
        },
        reward: {
          creator: destinationUser.publicKey,
          prover: Array.from(new Uint8Array(32).fill(3)),
          tokens: [
            {
              token: Array.from(new PublicKey(ethers.ZeroAddress).toBytes()),
              amount: new BN(usdcAmount(1)),
            },
          ],
          nativeAmount: new BN(0),
          deadline: new BN(0),
        },
      })
      .accountsStrict({
        payer: solver.publicKey,
        solver: solver.publicKey,
        executionAuthority,
        dispatchAuthority,
        mailboxProgram: MAILBOX_ID_TESTNET,
        outboxPda,
        splNoopProgram: SPL_NOOP_ID,
        uniqueMessage: Keypair.generate().publicKey,
        intentFulfillmentMarker,
        dispatchedMessagePda,
        splTokenProgram: TOKEN_PROGRAM_ID,
        splToken2022Program: TOKEN_2022_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    const blockhash = await connection.getLatestBlockhash();
    const fulfillTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: feePayer.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [fulfillIx],
      }).compileToV0Message()
    );

    fulfillTx.sign([solver]);
    const fulfillTxSignature = await connection.sendRawTransaction(
      fulfillTx.serialize()
    );
    await connection.confirmTransaction(
      { signature: fulfillTxSignature, ...blockhash },
      "confirmed"
    );

    const accountInfo = await connection.getAccountInfo(
      intentFulfillmentMarker
    );
    expect(accountInfo?.data.length).to.be.greaterThan(0);
  });

  it("proves and withdraws on EVM", async () => {
    const solverEvmAddress = await solverEvm.getAddress();

    // simulate prover writing the mapping
    await testProver
      .connect(creator)
      .addProvenIntent(intentHashHex, solverEvmAddress);

    // balances before
    const solverBalanceBefore = await usdc.balanceOf(solverEvmAddress);
    const vaultAddress = await intentSource.intentVaultAddress({
      route,
      reward,
    });
    const vaultBalance = await usdc.balanceOf(vaultAddress);
    expect(vaultBalance).to.equal(evmUsdcAmount(1));

    // withdraw
    const routeHash = ethers.keccak256(encodeRoute(route));
    await intentSource.connect(solverEvm).withdrawRewards(routeHash, reward);

    // after balances
    const solverBalanceAfter = await usdc.balanceOf(solverEvmAddress);
    expect(solverBalanceAfter - solverBalanceBefore).to.equal(evmUsdcAmount(1));

    // vault should be self-destructed, hence balance 0
    expect(await hardhatEthers.provider.getCode(vaultAddress)).to.equal("0x");
  });
});
