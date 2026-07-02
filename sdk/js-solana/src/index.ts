import { createRequire } from "node:module";

import type { PythLazerSolanaContract } from "./idl/pyth-lazer-solana-contract.js";

export type { PythLazerSolanaContract } from "./idl/pyth-lazer-solana-contract.js";

// The IDL is loaded from its JSON at runtime rather than via a static
// `import ... with { type: "json" }`. The ts-duality/SWC build strips the JSON
// import attribute, which makes the emitted ESM throw
// `ERR_IMPORT_ATTRIBUTE_MISSING` under strict ESM. `createRequire` works
// identically in both the ESM and CJS build outputs and needs no attribute.
// The JSON is shipped into `dist/**/idl/` by `ts-duality --copyOtherFiles`.
const nodeRequire = createRequire(import.meta.url);
export const PYTH_LAZER_SOLANA_CONTRACT_IDL = nodeRequire(
  "./idl/pyth-lazer-solana-contract.json",
) as PythLazerSolanaContract;

export { createEd25519Instruction } from "./ed25519.js";
