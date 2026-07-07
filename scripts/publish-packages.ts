/**
 * Bun doesn't have a recursive publish feature like PNPM did.
 * As a result, there isn't a built-in way to publish all public packages
 * in a monorepo like our.
 * As such, this script exists to gather all packages,
 * determine if the version marked has already been published and,
 * if it hasn't, publishes it to NPM
 */
/** biome-ignore-all lint/suspicious/noConsole: it's a script file, we need to output to the console */
/** biome-ignore-all lint/nursery/noUndeclaredEnvVars: We don't care about turbo in this file */
/** biome-ignore-all lint/style/noProcessEnv: it's a script file, it needs access to the env */

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
const pkg = await rootPkgFile.json();

const packagesMap = await mapWorkspaces({ cwd: repoRoot, pkg });

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

  const result = Bun.spawnSync(
    [
      "npm",
      "publish",
      "--provenance",
      "--access",
      "public",
      dryRun ? "--dry-run" : "",
    ].filter(Boolean),
    { cwd: p.packagePath },
  );

  if (result.exitCode !== 0) {
    hasErrors = true;
    console.error("🚨 an error occurred when publishing", p.name);
    console.error(result.stdout.toString("utf-8"));
    console.error(result.stderr.toString("utf-8"));
  } else {
    console.info(`✅ ${p.name}@${version} was published`);
  }
}

if (hasErrors) {
  console.error("⚠️  Some packages failed to publish");
  process.exit(1);
}
