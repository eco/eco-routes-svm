/* tests/evm-to-svm.e2e.ts
 *
 * Scenario EVM → SVM → EVM
 *   1. publish&fund intent on EVM (Hardhat chain)
 *   2. fulfill on Solana dev/test-net
 *   3. prove + withdraw on EVM
 */
import "dotenv/config";
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
import {
  createAssociatedTokenAccount,
  createMint,
  getAssociatedTokenAddressSync,
  mintTo,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";
import { ethers, hexlify, JsonRpcProvider, Signer } from "ethers";
import {
  TestERC20__factory,
  UniversalSource__factory,
  TestProver__factory,
  UniversalSource,
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
  TEST_USDC_ADDRESS_TESTNET,
  INTENT_SOURCE_ADDRESS_TESTNET,
  USDC_DECIMALS,
  STORAGE_PROVER_ADDRESS_TESTNET,
  INBOX_ADDRESS_TESTNET,
} from "./constants";
import {
  evmUsdcAmount,
  encodeRoute,
  encodeReward,
  hashIntent,
  Route,
  Reward,
  TokenAmount,
  addressToBytes32,
} from "./evmUtils";
import { loadKeypairFromFile, usdcAmount } from "./utils";
import ecoRoutesIdl from "../../target/idl/eco_routes.json";

const solver = loadKeypairFromFile("../../keys/program_auth.json"); // SVM solver key
const connection = new Connection(TESTNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(solver), {
  commitment: "confirmed",
});
const program = new Program(
  ecoRoutesIdl as anchor.Idl,
  provider
) as Program<EcoRoutes>;

let intentHashBytes!: Uint8Array;
let route!: Route;
let reward!: Reward;

// Re-use same 32-byte salt cross-chains
const salt = anchorUtils.bytes.utf8.encode("evm-svm-e2e".padEnd(32, "\0"));

describe("EVM → SVM e2e", () => {
  let usdc: TestERC20;
  let intentSource: UniversalSource;
  let testProver: TestProver;
  let creatorEvm!: Signer;
  let solverEvm!: Signer;
  let intentHashHex!: string;
  let mockSvmUsdcMint: PublicKey;

  before("creates a testet usdc and mints to solver", async () => {
    const usdcMint = Keypair.generate();
    mockSvmUsdcMint = usdcMint.publicKey;

    await createMint(
      connection,
      solver, // payer
      solver.publicKey, // mint authority
      null, // freeze authority
      USDC_DECIMALS, // decimals
      usdcMint, // mint keypair
      {
        commitment: "confirmed",
      }
    );

    const ata = await createAssociatedTokenAccount(
      connection,
      solver,
      mockSvmUsdcMint,
      solver.publicKey,
      {
        commitment: "confirmed",
      }
    );

    await mintTo(
      connection,
      solver,
      mockSvmUsdcMint,
      ata,
      solver,
      usdcAmount(1000), // amount to mint
      [],
      {
        commitment: "confirmed",
      }
    );
  });

  it("publishes & funds an intent on EVM", async () => {
    const l2Provider = new JsonRpcProvider(process.env.RPC_SEPOLIA);
    creatorEvm = new ethers.Wallet(process.env.PK_CREATOR!, l2Provider);
    solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

    // mock USDC
    usdc = TestERC20__factory.connect(TEST_USDC_ADDRESS_TESTNET, creatorEvm);

    // IntentSource contract
    intentSource = UniversalSource__factory.connect(
      INTENT_SOURCE_ADDRESS_TESTNET,
      creatorEvm
    );

    // TestProver and give its address to reward.prover
    testProver = TestProver__factory.connect(
      STORAGE_PROVER_ADDRESS_TESTNET,
      creatorEvm
    );

    const creatorAddress = await creatorEvm.getAddress();
    const intentSourceAddress = await intentSource.getAddress();
    const usdcAddress = await usdc.getAddress();
    const testProverAddress = await testProver.getAddress();

    // build Route + Reward structs
    const saltHex = "0x" + Buffer.from(salt).toString("hex");

    const usdcMintBytes32 = "0x" + mockSvmUsdcMint.toBuffer().toString("hex");
    const routeTokens: TokenAmount[] = [
      {
        token: usdcMintBytes32,
        amount: BigInt(usdcAmount(5)),
      },
    ];

    const evmUsdcBytes32 = hexlify(addressToBytes32(usdcAddress));
    const rewardTokens: TokenAmount[] = [
      { token: evmUsdcBytes32, amount: evmUsdcAmount(5) },
    ];

    const inboxBytes32 = hexlify(addressToBytes32(INBOX_ADDRESS_TESTNET));
    const creatorBytes32 = hexlify(addressToBytes32(creatorAddress));
    const proverBytes32 = hexlify(addressToBytes32(testProverAddress));

    route = {
      salt: saltHex,
      source: EVM_DOMAIN_ID,
      destination: SOLANA_DOMAIN_ID,
      inbox: inboxBytes32,
      tokens: routeTokens,
      calls: [],
    };

    reward = {
      creator: creatorBytes32,
      prover: proverBytes32,
      deadline: BigInt(0),
      nativeValue: BigInt(0),
      tokens: rewardTokens,
    };

    // allow IntentSource to pull USDC + publishAndFund
    await usdc.approve(intentSourceAddress, evmUsdcAmount(10));
    const publishTx = await intentSource.publishAndFund(
      { reward, route },
      true
    );
    await publishTx.wait();

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

    const evmUsdcAddress = await usdc.getAddress();

    const uniqueMessage = Keypair.generate();

    const executionAuthorityAta = await createAssociatedTokenAccount(
      connection,
      solver,
      mockSvmUsdcMint,
      executionAuthority,
      { commitment: "confirmed" },
      undefined,
      undefined,
      true
    );

    const solverAta = getAssociatedTokenAddressSync(
      mockSvmUsdcMint,
      solver.publicKey
    );

    const remainingAccounts = [
      { pubkey: mockSvmUsdcMint, isWritable: false, isSigner: false },
      { pubkey: solverAta, isWritable: true, isSigner: false },
      { pubkey: executionAuthorityAta, isWritable: true, isSigner: false },
    ];

    const fulfillIx = await program.methods
      .fulfillIntent({
        intentHash: Array.from(intentHashBytes),
        route: {
          salt: Array.from(salt),
          sourceDomainId: EVM_DOMAIN_ID,
          destinationDomainId: SOLANA_DOMAIN_ID,
          inbox: Array.from(addressToBytes32(INBOX_ADDRESS_TESTNET)),
          tokens: [
            {
              token: Array.from(mockSvmUsdcMint.toBytes()),
              amount: new BN(usdcAmount(5)),
            },
          ],
          calls: [],
        },
        reward: {
          creator: new PublicKey(
            addressToBytes32(await creatorEvm.getAddress())
          ),
          prover: Array.from(addressToBytes32(STORAGE_PROVER_ADDRESS_TESTNET)),
          tokens: [
            {
              token: Array.from(addressToBytes32(evmUsdcAddress)),
              amount: new BN(usdcAmount(8)),
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
        uniqueMessage: uniqueMessage.publicKey,
        intentFulfillmentMarker,
        dispatchedMessagePda,
        splTokenProgram: TOKEN_PROGRAM_ID,
        splToken2022Program: TOKEN_2022_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .remainingAccounts(remainingAccounts)
      .instruction();

    const blockhash = await connection.getLatestBlockhash();
    const fulfillTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: solver.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [fulfillIx],
      }).compileToV0Message()
    );

    fulfillTx.sign([solver, uniqueMessage]);
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
    const l2Provider = new JsonRpcProvider(process.env.RPC_SEPOLIA);
    const solverEvmAddress = await solverEvm.getAddress();

    // simulate prover writing the mapping
    await testProver
      .connect(creatorEvm)
      .addProvenIntent(intentHashHex, solverEvmAddress);

    // vault address
    const vaultAddress = await intentSource[
      "intentVaultAddress(((bytes32,uint256,uint256,address,(address,uint256)[],(address,bytes,uint256)[]),(address,address,uint256,uint256,(address,uint256)[])))"
    ]({
      route,
      reward,
    });

    // withdraw
    const routeHash = ethers.keccak256(encodeRoute(route));
    await intentSource
      .connect(solverEvm)
      [
        "withdrawRewards(bytes32,(address,address,uint256,uint256,(address,uint256)[]))"
      ](routeHash, reward);

    // after balances
    const solverBalanceAfter = await usdc.balanceOf(solverEvmAddress);
    expect(solverBalanceAfter).to.equal(evmUsdcAmount(5));

    // vault should be self-destructed, hence balance 0
    expect(await l2Provider.provider.getCode(vaultAddress)).to.equal("0x");
  });
});
