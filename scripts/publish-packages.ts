/**
 * Bun doesn't have a recursive publish feature like PNPM did.
 * As a result, there isn't a built-in way to publish all public packages
 * in a monorepo like our.
 * As such, this script exists to gather all packages,
 * determine if the version marked has already been published and,
 * if it hasn't, publishes it to NPM.
 *
 * Publishing is a two-step dance:
 *
 *   1. `bun pm pack` builds the tarball. Bun owns the `catalog:`/`workspace:`
 *      protocols (defined in the root package.json), so it resolves them to
 *      concrete versions in the packed `package.json`. `npm` has no knowledge
 *      of those protocols and would otherwise pack the literal strings straight
 *      into the published `dependencies`, making the package uninstallable in
 *      downstream workspaces.
 *   2. `npm publish <tarball>` uploads that pre-built tarball. We go through
 *      `npm` (rather than `bun publish`) because npm supports the
 *      provenance/OIDC trusted-publishing flow this repo relies on and
 *      `bun publish` does not. Provenance is derived from the CI environment
 *      (repo + commit + OIDC) and the tarball digest, so publishing a
 *      pre-packed tarball produces a valid attestation.
 */
/** biome-ignore-all lint/suspicious/noConsole: it's a script file, we need to output to the console */
/** biome-ignore-all lint/nursery/noUndeclaredEnvVars: We don't care about turbo in this file */
/** biome-ignore-all lint/style/noProcessEnv: it's a script file, it needs access to the env */

import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import mapWorkspaces from "@npmcli/map-workspaces";
import Bun from "bun";
import type { PackageJson } from "type-fest";

const strToBool = (str: string) => str.toLowerCase().trim() === "true";

const dryRun = strToBool(process.env.DRY_RUN ?? "");

if (dryRun) {
  console.info("📢 Dry run has been enabled");
}

const repoRoot = path.join(import.meta.dirname, "../");
const rootPkgFile = Bun.file(path.join(repoRoot, "package.json"));
const rootPkg = await rootPkgFile.json();

const packagesMap = await mapWorkspaces({ cwd: repoRoot, pkg: rootPkg });

const packages: { name: string; packagePath: string; pkg: PackageJson }[] = [];

for (const [name, packagePath] of packagesMap.entries()) {
  const pkg = await Bun.file(path.join(packagePath, "package.json")).json();
  packages.push({ name, packagePath, pkg });
}

const publicPackages = packages.filter((p) => !p.pkg.private);

let hasErrors = false;

for (const p of publicPackages) {
  const viewResult = Bun.spawnSync(["bun", "pm", "view", p.name, "version"]);

  // If the package doesn't exist on NPM yet, viewResult will have non-zero exit code
  // In that case, publishedVersion will be empty string and we'll publish it
  let publishedVersion = "";
  if (viewResult.exitCode === 0) {
    publishedVersion = viewResult.stdout.toString("utf-8").trim();
  }

  const {
    pkg: { version },
  } = p;

  // can't publish to NPM if the package.json is malformed and is missing its version field
  if (!version) {
    console.info(
      "unable to publish",
      p.name,
      'because it is missing a "version" property in its package.json file',
    );
    continue;
  }
  if (version === publishedVersion) {
    console.info(
      "✋🏼 skipping publishing",
      p.name,
      'because there has been no change to its "version" property',
    );
    continue;
  }

  const packDir = await mkdtemp(path.join(tmpdir(), "pyth-lazer-publish-"));

  try {
    // `bun pm pack` resolves Bun's `catalog:`/`workspace:` protocols into
    // concrete versions in the packed package.json (see the module comment).
    const packResult = Bun.spawnSync(
      ["bun", "pm", "pack", "--destination", packDir],
      { cwd: p.packagePath },
    );

    if (packResult.exitCode !== 0) {
      hasErrors = true;
      console.error("🚨 an error occurred when packing", p.name);
      console.error(packResult.stdout.toString("utf-8"));
      console.error(packResult.stderr.toString("utf-8"));
      continue;
    }

    const tarballs = (await readdir(packDir)).filter((f) => f.endsWith(".tgz"));
    if (tarballs.length !== 1) {
      hasErrors = true;
      console.error(
        "🚨 expected exactly one tarball for",
        p.name,
        "but found",
        tarballs.join(", ") || "none",
      );
      continue;
    }
    const tarballPath = path.join(packDir, tarballs[0]);

    const result = Bun.spawnSync(
      [
        "npm",
        "publish",
        tarballPath,
        "--provenance",
        "--access",
        "public",
        dryRun ? "--dry-run" : "",
      ].filter(Boolean),
    );

    if (result.exitCode !== 0) {
      hasErrors = true;
      console.error("🚨 an error occurred when publishing", p.name);
      console.error(result.stdout.toString("utf-8"));
      console.error(result.stderr.toString("utf-8"));
    } else {
      console.info(`✅ ${p.name}@${version} was published`);
    }
  } finally {
    await rm(packDir, { force: true, recursive: true });
  }
}

if (hasErrors) {
  console.error("⚠️  Some packages failed to publish");
  process.exit(1);
}
