import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import NodeWallet from "@coral-xyz/anchor/dist/cjs/nodewallet";
import { readFileSync } from "fs";
import yargs from "yargs/yargs";
import * as pythLazerSolanaContractIdl from "../target/idl/pyth_lazer_solana_contract.json";
import { PythLazerSolanaContract } from "../target/types/pyth_lazer_solana_contract";

async function main() {
  let argv = await yargs(process.argv.slice(2))
    .options({
      "keypair-path": { demandOption: true, type: "string" },
      message: { demandOption: true, type: "string" },
      url: { demandOption: true, type: "string" },
    })
    .parse();

  const keypair = anchor.web3.Keypair.fromSecretKey(
    new Uint8Array(JSON.parse(readFileSync(argv.keypairPath, "ascii"))),
  );

  const wallet = new NodeWallet(keypair);
  const connection = new anchor.web3.Connection(argv.url, {
    commitment: "confirmed",
  });
  const provider = new anchor.AnchorProvider(connection, wallet);

  const program: Program<PythLazerSolanaContract> = new Program(
    pythLazerSolanaContractIdl as PythLazerSolanaContract,
    provider,
  );

  await program.methods
    .verifyEcdsaMessage(Buffer.from(argv.message, "hex"))
    .accounts({
      payer: wallet.publicKey,
    })
    .rpc();

  console.log("message is valid");
}

main();
