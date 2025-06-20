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
import { HYPER_PROVER_ADDRESS, MAINNET_RPC } from "../constants";
import { addressToBytes32Hex, hex32ToNums } from "../evmUtils";

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

  const evmHyperProverBytes = hex32ToNums(
    addressToBytes32Hex(HYPER_PROVER_ADDRESS)
  );

  const setProverIx = await program.methods
    .setAuthorizedProver({
      newProver: evmHyperProverBytes,
    })
    .accountsStrict({
      ecoRoutes: ecoRoutesPda,
      authority: authorityKp.publicKey,
      systemProgram: SystemProgram.programId,
    })
    .instruction();

  const blockhash = await connection.getLatestBlockhash();

  try {
    const setProverTx = new VersionedTransaction(
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
          setProverIx,
        ],
      }).compileToV0Message()
    );

    setProverTx.sign([authorityKp]);
    const setProverTxSignature = await connection.sendRawTransaction(
      setProverTx.serialize()
    );

    await connection.confirmTransaction(
      { signature: setProverTxSignature, ...blockhash },
      "confirmed"
    );

    console.log("Set Prover tx sig :", setProverTxSignature);
  } catch (error) {
    console.log("Error setting an eco routes prover:", error);
    throw error;
  }
})();
