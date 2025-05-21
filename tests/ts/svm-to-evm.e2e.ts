/*
 *     1. Publish + Fund intent on Solana  (call to devnet program)
 *     2. Fulfil intent on EVM             (mocked / TODO)
 *     3. Claim on Solana                  (call to devnet program)
 */

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
  DEVNET_RPC,
  EVM_DOMAIN_ID,
  SOLANA_DOMAIN_ID,
  USDC_DECIMALS,
} from "./constants";
import { ethers } from "ethers";

anchor.setProvider(
  new AnchorProvider(
    new Connection(DEVNET_RPC, "confirmed"),
    new anchor.Wallet(Keypair.generate()),
    { commitment: "confirmed" }
  )
);

const provider = anchor.getProvider() as AnchorProvider;
const connection = provider.connection;
const program = anchor.workspace.EcoRoutes as Program<EcoRoutes>;

const feePayer = loadKeypairFromFile("../../keys/program_auth.json");
const sourceUser = Keypair.generate();
const solver = Keypair.generate();

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
      feePayer, // payer
      feePayer.publicKey, // mint authority
      null, // freeze authority
      USDC_DECIMALS, // decimals
      usdcMint, // mint keypair
      {
        commitment: "confirmed",
      }
    );

    const ata = await createAssociatedTokenAccount(
      connection,
      feePayer,
      mockSvmUsdcMint,
      sourceUser.publicKey,
      {
        commitment: "confirmed",
      }
    );

    await mintTo(
      connection,
      feePayer,
      mockSvmUsdcMint,
      ata,
      feePayer,
      usdcAmount(1000), // amount to mint
      [],
      {
        commitment: "confirmed",
      }
    );
  });

  it("Publish + Fund intent on Solana", async () => {
    route = {
      salt,
      sourceDomainId: SOLANA_DOMAIN_ID,
      destinationDomainId: EVM_DOMAIN_ID,
      inbox: new Uint8Array(32).fill(9),
      tokens: [
        {
          token: Array.from(mockSvmUsdcMint.toBytes()), // TODO: replace with ETH USDC address
          amount: new BN(usdcAmount(5)),
        },
      ],
      calls: [],
    };

    reward = {
      creator: sourceUser.publicKey.toBytes(),
      prover: new Uint8Array(32).fill(3),
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
        inbox: Array.from(route.inbox),
        routeTokens: route.tokens,
        calls: [],
        rewardTokens: reward.tokens,
        nativeReward: reward.nativeAmount,
        deadline: reward.deadline,
      })
      .accountsStrict({
        intent,
        creator: sourceUser.publicKey,
        payer: feePayer.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    let blockhash = await connection.getLatestBlockhash();
    let publishIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: feePayer.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [publishIx],
      }).compileToV0Message()
    );
    publishIntentTx.sign([feePayer, sourceUser]);

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
        funder: sourceUser.publicKey,
        payer: feePayer.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    /* SPL transfer of USDC to Intent */
    const fundSplIx = await program.methods
      .fundIntentSpl({
        intentHash: Array.from(intentHash) as number[],
        tokenToFund: 0, // USDC reward index
      })
      .accountsStrict({
        intent,
        funder: sourceUser.publicKey,
        payer: feePayer.publicKey,
        systemProgram: SystemProgram.programId,
        funderToken: getAssociatedTokenAddressSync(
          mockSvmUsdcMint,
          sourceUser.publicKey
        ),
        vault,
        mint: mockSvmUsdcMint,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    blockhash = await connection.getLatestBlockhash();
    let fundIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: feePayer.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [fundNativeIx, fundSplIx],
      }).compileToV0Message()
    );
    publishIntentTx.sign([feePayer, sourceUser]);

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
    /**
     * TODO
     */
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
        claimer: solver.publicKey,
        payer: solver.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    // spl claim ix
    const claimSplIx = await program.methods
      .claimIntentSpl({
        intentHash: Array.from(intentHash) as number[],
        tokenToClaim: 0,
      })
      .accountsStrict({
        intent,
        claimer: solver.publicKey,
        payer: solver.publicKey,
        systemProgram: SystemProgram.programId,
        vault,
        claimerToken: getAssociatedTokenAddressSync(
          mockSvmUsdcMint,
          solver.publicKey
        ),
        mint: mockSvmUsdcMint,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    const blockhash = await connection.getLatestBlockhash();
    let claimIntentTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: feePayer.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [claimNativeIx, claimSplIx],
      }).compileToV0Message()
    );
    claimIntentTx.sign([solver]);

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
