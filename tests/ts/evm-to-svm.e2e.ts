/*
 *     1. Init + Fund intent on EVM   (mocked / TODO)
 *     2. Fulfil intent on Solana     (call to devnet program)
 *     3. Claim on EVM                (TODO)
 */

import {
  Keypair,
  Connection,
  PublicKey,
  LAMPORTS_PER_SOL,
  VersionedTransaction,
  TransactionMessage,
} from "@solana/web3.js";
import { AnchorProvider, BN, Program } from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import { expect } from "chai";
import { EcoRoutes } from "../../target/types/eco_routes";
import { before, describe, it } from "node:test";
import { TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { generateIntentHash, loadKeypairFromFile, usdcAmount } from "./utils";
import bs58 from "bs58";

const MAILBOX_ID = new PublicKey(
  "E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi"
);
const SPL_NOOP_ID = new PublicKey(
  "noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV"
);
const EVM_DOMAIN_ID = 11155111;
const SOLANA_DOMAIN_ID = 1399811149;
const DEVNET_RPC = "https://api.devnet.solana.com";
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
const solver = Keypair.generate();
const destinationUser = Keypair.generate();
const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

let salt = anchor.utils.bytes.utf8.encode("evm-test-salt".padEnd(32, "\0"));
let intentHash: Uint8Array;

async function airdrop(pubkey: PublicKey) {
  const airdropSignature = await connection.requestAirdrop(
    pubkey,
    1 * LAMPORTS_PER_SOL
  );
  await connection.confirmTransaction(airdropSignature, "confirmed");
}

describe("EVM -> SVM e2e", () => {
  // Init + fund an intent on EVM
  it("Init + Fund intent on EVM (mock only)", async () => {
    // Temp hard-coded structures the Solana side will expect.
    // TODO: replace with intent creation and funding on EVM.
    const route = {
      salt,
      sourceDomainId: EVM_DOMAIN_ID,
      destinationDomainId: SOLANA_DOMAIN_ID,
      inbox: new Uint8Array(32).fill(7),
      tokens: [
        {
          token: USDC_MINT.toBytes(),
          amount: new BN(usdcAmount(5)),
        },
      ],
      calls: [],
    };

    const reward = {
      creator: new Uint8Array(32).fill(2),
      prover: new Uint8Array(32).fill(3),
      tokens: [
        {
          token: USDC_MINT.toBytes(),
          amount: new BN(usdcAmount(1)),
        },
      ],
      nativeAmount: new BN(0),
      deadline: new BN(0),
    };

    intentHash = generateIntentHash(route, reward);
    expect(intentHash.length).equals(32);
  });

  // Fulfil intent on Solana                                     */
  it("Fulfil intent on Solana", async () => {
    const executionAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("execution_authority"), salt],
      program.programId
    )[0];
    const dispatchAuthority = PublicKey.findProgramAddressSync(
      [Buffer.from("dispatch_authority")],
      program.programId
    )[0];
    const intentFulfillmentMarker = PublicKey.findProgramAddressSync(
      [Buffer.from("intent_fulfillment_marker"), intentHash],
      program.programId
    )[0];
    const outboxPda = PublicKey.findProgramAddressSync(
      [Buffer.from("hyperlane"), Buffer.from("-"), Buffer.from("outbox")],
      MAILBOX_ID
    )[0];
    const dispatchedMessagePda = PublicKey.findProgramAddressSync(
      [
        Buffer.from("hyperlane"),
        Buffer.from("-"),
        Buffer.from("dispatched_message"),
        Buffer.from("-"),
        Keypair.generate().publicKey.toBuffer(),
      ],
      MAILBOX_ID
    )[0];

    const fulfillIx = await program.methods
      .fulfillIntent({
        intentHash: Array.from(intentHash) as number[],
        route: {
          salt: Array.from(salt) as number[],
          sourceDomainId: EVM_DOMAIN_ID,
          destinationDomainId: SOLANA_DOMAIN_ID,
          inbox: Array.from(new Uint8Array(32).fill(7)) as number[],
          tokens: [
            {
              token: Array.from(USDC_MINT.toBytes()) as number[],
              amount: new BN(usdcAmount(5)),
            },
          ],
          calls: [] as {
            destination: number[];
            calldata: Buffer;
          }[],
        },
        reward: {
          creator: destinationUser.publicKey,
          prover: Array.from(new Uint8Array(32).fill(3)) as number[],
          tokens: [
            {
              token: Array.from(USDC_MINT.toBytes()) as number[],
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
        mailboxProgram: MAILBOX_ID,
        outboxPda,
        splNoopProgram: SPL_NOOP_ID,
        uniqueMessage: Keypair.generate().publicKey,
        intentFulfillmentMarker,
        dispatchedMessagePda,
        splTokenProgram: TOKEN_PROGRAM_ID,
        splToken2022Program: TOKEN_2022_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .instruction();

    const blockhash = await connection.getLatestBlockhash();
    const fulfillTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: solver.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [fulfillIx],
      }).compileToV0Message()
    );
    fulfillTx.sign([solver]);
    const fulfillTxSignature = await connection.sendRawTransaction(
      fulfillTx.serialize()
    );
    await connection.confirmTransaction(
      {
        signature: fulfillTxSignature,
        blockhash: blockhash.blockhash,
        lastValidBlockHeight: blockhash.lastValidBlockHeight,
      },
      "confirmed"
    );

    // verify intent fulfillment
    const intentFulfillmentMarkerAccountInfo = await connection.getAccountInfo(
      intentFulfillmentMarker
    );
    expect(intentFulfillmentMarkerAccountInfo.data.length).to.be.greaterThan(0);
  });

  it("Claim intent on EVM (TODO)", () => {
    // would claimIntent() once Hyperlane delivers message.
    expect(true).to.be.true;
  });
});
