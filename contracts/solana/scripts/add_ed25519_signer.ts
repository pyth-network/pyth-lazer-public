import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import NodeWallet from "@coral-xyz/anchor/dist/cjs/nodewallet";
import { readFileSync } from "fs";
import yargs from "yargs/yargs";
import * as pythLazerSolanaContractIdl from "../target/idl/pyth_lazer_solana_contract.json";
import { PythLazerSolanaContract } from "../target/types/pyth_lazer_solana_contract";

// Add a trusted signer or change its expiry time.
//
// Example:
// bun scripts/add_ed25519_signer.ts --url 'https://api.testnet.solana.com' \
//    --keypair-path .../key.json --trusted-signer HaXscpSUcbCLSnPQB8Z7H6idyANxp1mZAXTbHeYpfrJJ \
//    --expiry-time-seconds 2057930841
async function main() {
  let argv = await yargs(process.argv.slice(2))
    .options({
      "expiry-time-seconds": { demandOption: true, type: "number" },
      "keypair-path": { demandOption: true, type: "string" },
      "trusted-signer": { demandOption: true, type: "string" },
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
    .update(
      new anchor.web3.PublicKey(argv.trustedSigner),
      new anchor.BN(argv.expiryTimeSeconds),
    )
    .accounts({})
    .rpc();
  console.log("signer updated");
}

main();
