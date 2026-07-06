/**
 * Bun doesn't have a recursive publish feature like PNPM did.
 * As a result, there isn't a built-in way to publish all public packages
 * in a monorepo like our.
 * As such, this script exists to gather all packages,
 * determine if the version marked has already been published and,
 * if it hasn't, publishes it to NPM.
 *
 * Publishing goes through `npm publish` (rather than `bun publish`) because
 * npm supports the provenance/OIDC trusted-publishing flow this repo relies on
 * and `bun publish` does not. The downside is that `npm` has no knowledge of
 * Bun's `catalog:`/`workspace:` protocols, so it would pack those literal
 * strings straight into the published `dependencies` and make the package
 * uninstallable in downstream workspaces. To avoid that, we resolve those
 * protocols to concrete versions in each package.json right before publishing
 * and restore the original file afterwards.
 */
/** biome-ignore-all lint/suspicious/noConsole: it's a script file, we need to output to the console */
/** biome-ignore-all lint/nursery/noUndeclaredEnvVars: We don't care about turbo in this file */
/** biome-ignore-all lint/style/noProcessEnv: it's a script file, it needs access to the env */

import path from "node:path";
import mapWorkspaces from "@npmcli/map-workspaces";
import Bun from "bun";
import type { PackageJson } from "type-fest";

const DEPENDENCY_FIELDS = [
  "dependencies",
  "devDependencies",
  "optionalDependencies",
  "peerDependencies",
] as const;

type Catalog = Record<string, string>;

type Catalogs = {
  /** The default (unnamed) catalog from the root package.json `catalog` field. */
  catalog: Catalog;
  /** Named catalogs from the root package.json `catalogs` field. */
  catalogs: Record<string, Catalog>;
  /** Map of workspace package name -> its local version. */
  workspaceVersions: Map<string, string>;
};

/**
 * Resolve a single dependency specifier that may use Bun's `catalog:` or
 * `workspace:` protocol into a concrete, npm-publishable version range.
 * Specifiers using neither protocol are returned unchanged.
 */
const resolveSpecifier = (
  depName: string,
  specifier: string,
  { catalog, catalogs, workspaceVersions }: Catalogs,
): string => {
  if (specifier.startsWith("catalog:")) {
    const catalogName = specifier.slice("catalog:".length).trim();
    const table = catalogName === "" ? catalog : catalogs[catalogName];
    const resolved = table?.[depName];
    if (resolved === undefined) {
      throw new Error(
        `Unable to resolve "${depName}": "${specifier}" is not defined in ${
          catalogName === ""
            ? "the default catalog"
            : `the "${catalogName}" catalog`
        } of the root package.json`,
      );
    }
    return resolved;
  }

  if (specifier.startsWith("workspace:")) {
    const version = workspaceVersions.get(depName);
    if (version === undefined) {
      throw new Error(
        `Unable to resolve "${depName}": "${specifier}" refers to a workspace package that does not exist`,
      );
    }
    // e.g. `workspace:*` -> the version, `workspace:^`/`workspace:~` -> prefixed
    // version, `workspace:<range>` -> that range verbatim.
    const range = specifier.slice("workspace:".length);
    if (range === "" || range === "*") {
      return version;
    }
    if (range === "^" || range === "~") {
      return `${range}${version}`;
    }
    return range;
  }

  return specifier;
};

/**
 * Return a copy of `pkg` with every `catalog:`/`workspace:` specifier across
 * all dependency fields replaced by a concrete version. Throws if any such
 * specifier cannot be resolved, so we never publish an unresolved protocol.
 */
export const resolveWorkspaceProtocols = (
  pkg: PackageJson,
  catalogs: Catalogs,
): PackageJson => {
  const resolved: PackageJson = { ...pkg };
  for (const field of DEPENDENCY_FIELDS) {
    const deps = pkg[field];
    if (deps === undefined) {
      continue;
    }
    resolved[field] = Object.fromEntries(
      Object.entries(deps).map(([name, specifier]) => [
        name,
        typeof specifier === "string"
          ? resolveSpecifier(name, specifier, catalogs)
          : specifier,
      ]),
    );
  }
  return resolved;
};

if (import.meta.main) {
  const strToBool = (str: string) => str.toLowerCase().trim() === "true";

  const dryRun = strToBool(process.env.DRY_RUN ?? "");

  if (dryRun) {
    console.info("📢 Dry run has been enabled");
  }

  const repoRoot = path.join(import.meta.dirname, "../");
  const rootPkgFile = Bun.file(path.join(repoRoot, "package.json"));
  const rootPkg = await rootPkgFile.json();

  const packagesMap = await mapWorkspaces({ cwd: repoRoot, pkg: rootPkg });

  const packages: { name: string; packagePath: string; pkg: PackageJson }[] =
    [];

  for (const [name, packagePath] of packagesMap.entries()) {
    const pkg = await Bun.file(path.join(packagePath, "package.json")).json();
    packages.push({ name, packagePath, pkg });
  }

  const workspaceVersions = new Map<string, string>();
  for (const p of packages) {
    if (typeof p.pkg.version === "string") {
      workspaceVersions.set(p.name, p.pkg.version);
    }
  }

  const catalogs: Catalogs = {
    catalog: rootPkg.catalog ?? {},
    catalogs: rootPkg.catalogs ?? {},
    workspaceVersions,
  };

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

    // `npm` doesn't understand Bun's `catalog:`/`workspace:` protocols, so we
    // resolve them to concrete versions on disk for the duration of the publish
    // and restore the original package.json afterwards.
    const pkgJsonPath = path.join(p.packagePath, "package.json");
    const originalContents = await Bun.file(pkgJsonPath).text();
    await Bun.write(
      pkgJsonPath,
      `${JSON.stringify(resolveWorkspaceProtocols(p.pkg, catalogs), null, 2)}\n`,
    );

    try {
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
    } finally {
      await Bun.write(pkgJsonPath, originalContents);
    }
  }

  if (hasErrors) {
    console.error("⚠️  Some packages failed to publish");
    process.exit(1);
  }
}
