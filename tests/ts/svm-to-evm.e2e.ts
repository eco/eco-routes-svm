/*
 *     1. Publish + Fund intent on Solana  (call to devnet program)
 *     2. Fulfil intent on EVM             (mocked / TODO)
 *     3. Claim on Solana                  (call to devnet program)
 */
import "dotenv/config";
import {
  AnchorProvider,
  BN,
  Program,
  utils as anchorUtils,
} from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  VersionedTransaction,
  TransactionMessage,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccount,
  createMint,
  getAssociatedTokenAddressSync,
  mintTo,
} from "@solana/spl-token";
import { expect } from "chai";
import { EcoRoutes } from "../../target/types/eco_routes";
import { describe, it } from "node:test";
import { generateIntentHash, usdcAmount, loadKeypairFromFile } from "./utils";
import {
  EVM_DOMAIN_ID,
  INBOX_ADDRESS_TESTNET,
  SOLANA_DOMAIN_ID,
  STORAGE_PROVER_ADDRESS_TESTNET,
  TEST_USDC_ADDRESS_TESTNET,
  TESTNET_RPC,
  USDC_DECIMALS,
} from "./constants";
import { ethers, JsonRpcProvider } from "ethers";
import { Inbox__factory, TestERC20__factory } from "./evm-types";
import {
  addressToBytes32Hex,
  encodeReward,
  encodeRoute,
  evmUsdcAmount,
  hex32ToBytes,
} from "./evmUtils";
import ecoRoutesIdl from "../../target/idl/eco_routes.json";

const creatorSvm = loadKeypairFromFile("../../keys/program_auth.json");
const connection = new Connection(TESTNET_RPC, "confirmed");
const provider = new AnchorProvider(connection, new anchor.Wallet(creatorSvm), {
  commitment: "confirmed",
});
const program = new Program(
  ecoRoutesIdl as anchor.Idl,
  provider
) as Program<EcoRoutes>;

const salt = anchorUtils.bytes.utf8.encode("svm-to-evm-test".padEnd(32, "\0"));

/* these will be filled in step-1 */
let route: any, reward: any, intentHash: Uint8Array;
let mockSvmUsdcMint: PublicKey | undefined = undefined;

describe("SVM -> EVM e2e", () => {
  before("creates a testet usdc mint", async () => {
    const usdcMint = Keypair.generate();
    mockSvmUsdcMint = usdcMint.publicKey;

    await createMint(
      connection,
      creatorSvm, // payer
      creatorSvm.publicKey, // mint authority
      null, // freeze authority
      USDC_DECIMALS, // decimals
      usdcMint, // mint keypair
      {
        commitment: "confirmed",
      }
    );

    const ata = await createAssociatedTokenAccount(
      connection,
      creatorSvm,
      mockSvmUsdcMint,
      creatorSvm.publicKey,
      {
        commitment: "confirmed",
      }
    );

    await mintTo(
      connection,
      creatorSvm,
      mockSvmUsdcMint,
      ata,
      creatorSvm,
      usdcAmount(1000), // amount to mint
      [],
      {
        commitment: "confirmed",
      }
    );

    // we need the solver to have an associated token account for the Claim SPL ix later
    await createAssociatedTokenAccount(
      connection,
      creatorSvm,
      mockSvmUsdcMint,
      creatorSvm.publicKey,
      { commitment: "confirmed" }
    );
  });

  it("Publish + Fund intent on Solana", async () => {
    route = {
      salt,
      sourceDomainId: SOLANA_DOMAIN_ID,
      destinationDomainId: EVM_DOMAIN_ID,
      inbox: Array.from(addressToBytes32Hex(INBOX_ADDRESS_TESTNET)),
      tokens: [
        {
          token: Array.from(mockSvmUsdcMint.toBytes()),
          amount: new BN(usdcAmount(5)),
        },
      ],
      calls: [],
    };

    reward = {
      creator: creatorSvm.publicKey.toBytes(),
      prover: Array.from(addressToBytes32Hex(STORAGE_PROVER_ADDRESS_TESTNET)),
      tokens: [
        {
          token: Array.from(mockSvmUsdcMint.toBytes()),
          amount: new BN(usdcAmount(10)),
        },
      ],
      nativeAmount: new BN(0),
      deadline: new BN(0),
    };

    intentHash = generateIntentHash(route, reward);
    expect(intentHash.length).equals(32);

    const intent = PublicKey.findProgramAddressSync(
      [Buffer.from("intent"), intentHash],
      program.programId
    )[0];

    const vault = PublicKey.findProgramAddressSync(
      [Buffer.from("reward"), intentHash, mockSvmUsdcMint.toBytes()],
      program.programId
    )[0];

    const publishIx = await program.methods
      .publishIntent({
        salt: Array.from(salt) as number[],
        intentHash: Array.from(intentHash) as number[],
        destinationDomainId: EVM_DOMAIN_ID,
        inbox: route.inbox,
        routeTokens: route.tokens,
        calls: [],
        rewardTokens: reward.tokens,
        nativeReward: reward.nativeAmount,
        deadline: reward.deadline,
      })
      .accountsStrict({
        intent,
        creator: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    let blockhash = await connection.getLatestBlockhash();
    let publishIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: creatorSvm.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [publishIx],
      }).compileToV0Message()
    );
    publishIntentTx.sign([creatorSvm]);

    const publishIntentTxSignature = await connection.sendRawTransaction(
      publishIntentTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: publishIntentTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify intent published
    const intentAccountInfo = await connection.getAccountInfo(intent);
    expect(intentAccountInfo.data.length).to.be.greaterThan(0);

    // Native transfer to Intent PDA
    const fundNativeIx = await program.methods
      .fundIntentNative({
        intentHash: Array.from(intentHash) as number[],
      })
      .accountsStrict({
        intent,
        funder: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    /* SPL transfer of USDC to Intent */
    const fundSplIx = await program.methods
      .fundIntentSpl({
        intentHash: Array.from(intentHash) as number[],
        tokenIndex: 0, // USDC reward index
      })
      .accountsStrict({
        intent,
        funder: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
        funderToken: getAssociatedTokenAddressSync(
          mockSvmUsdcMint,
          creatorSvm.publicKey
        ),
        vault,
        mint: mockSvmUsdcMint,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    blockhash = await connection.getLatestBlockhash();
    let fundIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: creatorSvm.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [fundNativeIx, fundSplIx],
      }).compileToV0Message()
    );
    fundIntentTx.sign([creatorSvm]);

    const fundIntentTxSignature = await connection.sendRawTransaction(
      fundIntentTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: fundIntentTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify intent funded
    const intentAccount = await program.account.intent.fetch(intent);
    expect(intentAccount.status.funded).to.be.true;
  });

  it("Fulfil intent on EVM (mock Hyperlane)", async () => {
    const l2Provider = new JsonRpcProvider(process.env.RPC_SEPOLIA);
    const solverEvm = new ethers.Wallet(process.env.PK_SOLVER!, l2Provider);

    const usdcEvm = TestERC20__factory.connect(
      TEST_USDC_ADDRESS_TESTNET,
      solverEvm
    );

    const inbox = Inbox__factory.connect(INBOX_ADDRESS_TESTNET, solverEvm);
    const saltHex = "0x" + Buffer.from(salt).toString("hex");
    const inboxBytes32 = hex32ToBytes(
      addressToBytes32Hex(INBOX_ADDRESS_TESTNET)
    );
    const evmUsdcBytes32 = hex32ToBytes(
      addressToBytes32Hex(await usdcEvm.getAddress())
    );

    const routeSol = {
      salt: saltHex as `0x${string}`,
      source: SOLANA_DOMAIN_ID,
      destination: EVM_DOMAIN_ID,
      inbox: inboxBytes32,
      tokens: [
        {
          token: "0x" + Buffer.from(evmUsdcBytes32).toString("hex"),
          amount: BigInt(evmUsdcAmount(5)),
        },
      ],
      calls: [] as { target: string; data: string; value: bigint }[],
    };

    const usdcMintBytes32 = "0x" + mockSvmUsdcMint.toBuffer().toString("hex");
    const creatorBytes32 =
      "0x" + creatorSvm.publicKey.toBuffer().toString("hex");
    const rewardSol = {
      creator: creatorBytes32,
      prover: STORAGE_PROVER_ADDRESS_TESTNET,
      deadline: BigInt(0),
      nativeValue: BigInt(0),
      tokens: [
        {
          token: usdcMintBytes32,
          amount: BigInt(usdcAmount(10)),
        },
      ],
    };

    const routeHash = ethers.keccak256(encodeRoute(routeSol));
    const rewardHash = ethers.keccak256(encodeReward(rewardSol));
    const expectedHash = ethers.keccak256(
      ethers.solidityPacked(["bytes32", "bytes32"], [routeHash, rewardHash])
    );

    await usdcEvm
      .connect(solverEvm)
      .approve(INBOX_ADDRESS_TESTNET, evmUsdcAmount(5));

    await inbox.fulfillAndProve(
      routeSol,
      rewardHash,
      await solverEvm.getAddress(),
      expectedHash,
      STORAGE_PROVER_ADDRESS_TESTNET,
      "0x",
      { gasLimit: 4_000_000 }
    );

    const fulfilledMappingSlot = await inbox.fulfilled(expectedHash);
    expect(fulfilledMappingSlot).to.equal(await solverEvm.getAddress());
  });

  it("Claim intent on Solana", async () => {
    const intent = PublicKey.findProgramAddressSync(
      [Buffer.from("intent"), intentHash],
      program.programId
    )[0];

    const vault = PublicKey.findProgramAddressSync(
      [Buffer.from("reward"), intentHash, mockSvmUsdcMint.toBytes()],
      program.programId
    )[0];

    // native claim ix
    const claimNativeIx = await program.methods
      .claimIntentNative({
        intentHash: Array.from(intentHash) as number[],
      })
      .accountsStrict({
        intent,
        claimer: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    // spl claim ix
    const claimSplIx = await program.methods
      .claimIntentSpl({
        intentHash: Array.from(intentHash) as number[],
        tokenIndex: 0,
      })
      .accountsStrict({
        intent,
        claimer: creatorSvm.publicKey,
        payer: creatorSvm.publicKey,
        systemProgram: SystemProgram.programId,
        vault,
        claimerToken: getAssociatedTokenAddressSync(
          mockSvmUsdcMint,
          creatorSvm.publicKey
        ),
        mint: mockSvmUsdcMint,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    const blockhash = await connection.getLatestBlockhash();
    let claimIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: creatorSvm.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [claimNativeIx, claimSplIx],
      }).compileToV0Message()
    );
    claimIntentTx.sign([creatorSvm]);

    const claimIntentTxSignature = await connection.sendRawTransaction(
      claimIntentTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: claimIntentTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify status to be claimed
    const intentAccount = await program.account.intent.fetch(intent);
    const claimed = intentAccount.status.claimed[0];
    expect(claimed).to.be.true;
  });
});
