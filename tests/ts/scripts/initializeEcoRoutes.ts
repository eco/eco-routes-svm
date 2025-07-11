import "dotenv/config";
import {
  Connection,
  PublicKey,
  SystemProgram,
  VersionedTransaction,
  TransactionMessage,
  ComputeBudgetProgram,
} from "@solana/web3.js";
import { AnchorProvider, Program } from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import { EcoRoutes } from "../../../target/types/eco_routes";
import ecoRoutesIdl from "../../../target/idl/eco_routes.json";
import { loadKeypairFromFile } from "../utils";
import { MAINNET_RPC } from "../constants";

const authorityKp = loadKeypairFromFile(
  "../../../keys/program_auth_mainnet.json"
);
const connection = new Connection(MAINNET_RPC, "confirmed");
const provider = new AnchorProvider(
  connection,
  new anchor.Wallet(authorityKp),
  {
    commitment: "confirmed",
  }
);
const program = new Program(
  ecoRoutesIdl as anchor.Idl,
  provider
) as Program<EcoRoutes>;

(async () => {
  const ecoRoutesPda = PublicKey.findProgramAddressSync(
    [Buffer.from("eco_routes")],
    program.programId
  )[0];

  const ecoRoutesProverBytes = PublicKey.findProgramAddressSync(
    [Buffer.from("dispatch_authority")],
    program.programId
  )[0].toBytes();

  const initializeEcoRoutesIx = await program.methods
    .initializeEcoRoutes({
      prover: Array.from(ecoRoutesProverBytes),
    })
    .accountsStrict({
      ecoRoutes: ecoRoutesPda,
      authority: authorityKp.publicKey,
      systemProgram: SystemProgram.programId,
    })
    .instruction();

  const blockhash = await connection.getLatestBlockhash();

  try {
    const initEcoRoutesTx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: authorityKp.publicKey,
        recentBlockhash: blockhash.blockhash,
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({
            units: 1_000_000,
          }),
          ComputeBudgetProgram.setComputeUnitPrice({
            microLamports: 150_000,
          }),
          initializeEcoRoutesIx,
        ],
      }).compileToV0Message()
    );

    initEcoRoutesTx.sign([authorityKp]);
    const initEcoRoutesTxSignature = await connection.sendRawTransaction(
      initEcoRoutesTx.serialize()
    );

    await connection.confirmTransaction(
      { signature: initEcoRoutesTxSignature, ...blockhash },
      "confirmed"
    );

    console.log("Init Eco Routes tx sig :", initEcoRoutesTxSignature);
  } catch (error) {
    console.log("Error during eco routes initializtion:", error);
    throw error;
  }
})();
