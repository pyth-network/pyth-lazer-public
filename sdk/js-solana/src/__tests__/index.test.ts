import { describe, expect, it } from "bun:test";

const EXPECTED_ADDRESS = "pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt";

// This is a packaging smoke test: it imports the *built* dist artifacts (not
// the TS source) because the bug being guarded against is that the published
// build shipped no runtime IDL value — `PYTH_LAZER_SOLANA_CONTRACT_IDL`
// resolved to `undefined` in both CJS and ESM. A source-level import would
// hide the regression since the bundler transpiles the JSON import cleanly.
describe("PYTH_LAZER_SOLANA_CONTRACT_IDL (built package)", () => {
  it("is defined with the expected program address in the ESM build", async () => {
    const esm = await import("../../dist/esm/index.mjs");
    expect(esm.PYTH_LAZER_SOLANA_CONTRACT_IDL).toBeDefined();
    expect(esm.PYTH_LAZER_SOLANA_CONTRACT_IDL.address).toBe(EXPECTED_ADDRESS);
  });

  it("is defined with the expected program address in the CJS build", async () => {
    const cjs = await import("../../dist/cjs/index.cjs");
    expect(cjs.PYTH_LAZER_SOLANA_CONTRACT_IDL).toBeDefined();
    expect(cjs.PYTH_LAZER_SOLANA_CONTRACT_IDL.address).toBe(EXPECTED_ADDRESS);
  });
});
