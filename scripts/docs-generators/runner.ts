#!/usr/bin/env npx tsx

/**
 * Documentation Generator Runner
 *
 * Orchestrates documentation generation across all configured libraries.
 * Reads config.json and runs appropriate generators based on what changed.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/runner.ts                    # Generate all enabled docs
 *   npx tsx scripts/docs-generators/runner.ts --check            # Check for drift (CI mode)
 *   npx tsx scripts/docs-generators/runner.ts --library lume     # Generate specific library
 *   npx tsx scripts/docs-generators/runner.ts --list             # List all configured generators
 *   npx tsx scripts/docs-generators/runner.ts --changed          # Only run for changed files (CI)
 */

import { execSync, spawnSync } from "child_process";
import * as fs from "fs";
import * as path from "path";

// ============================================================================
// Types
// ============================================================================

interface GeneratorOutput {
  type: string;
  outputFile: string;
  extractCommand: string | null;
}

interface GeneratorConfig {
  name: string;
  language: string;
  sourcePath: string;
  docsOutputPath: string;
  generatorScript: string;
  watchPaths: string[];
  buildCommand: string | null;
  buildDirectory: string;
  extractionMethod: string;
  outputs: GeneratorOutput[];
  enabled: boolean;
  notes?: string;
}

interface Config {
  description: string;
  generators: Record<string, GeneratorConfig>;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, "../..");
const CONFIG_PATH = path.join(__dirname, "config.json");

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);

  // Parse arguments
  const checkOnly = args.includes("--check") || args.includes("--check-only");
  const listOnly = args.includes("--list");
  const changedOnly = args.includes("--changed");
  const libraryIndex = args.indexOf("--library");
  const specificLibrary = libraryIndex !== -1 ? args[libraryIndex + 1] : null;

  console.log("📚 Documentation Generator Runner");
  console.log("==================================\n");

  // Load config
  if (!fs.existsSync(CONFIG_PATH)) {
    console.error(`❌ Config file not found: ${CONFIG_PATH}`);
    process.exit(1);
  }

  const config: Config = JSON.parse(fs.readFileSync(CONFIG_PATH, "utf-8"));

  // List mode
  if (listOnly) {
    listGenerators(config);
    return;
  }

  // Determine which generators to run
  let generatorsToRun: string[] = [];

  if (specificLibrary) {
    if (!config.generators[specificLibrary]) {
      console.error(`❌ Unknown library: ${specificLibrary}`);
      console.log(
        "\nAvailable libraries:",
        Object.keys(config.generators).join(", ")
      );
      process.exit(1);
    }
    generatorsToRun = [specificLibrary];
  } else if (changedOnly) {
    generatorsToRun = getChangedGenerators(config);
    if (generatorsToRun.length === 0) {
      console.log("✅ No documentation-related changes detected.");
      return;
    }
  } else {
    // Run all enabled generators
    generatorsToRun = Object.entries(config.generators)
      .filter(([_, cfg]) => cfg.enabled)
      .map(([key, _]) => key);
  }

  if (generatorsToRun.length === 0) {
    console.log("⚠️  No generators to run. Enable generators in config.json.");
    return;
  }

  console.log(`📋 Generators to run: ${generatorsToRun.join(", ")}\n`);

  // Run generators
  let hasErrors = false;

  for (const generatorKey of generatorsToRun) {
    const generator = config.generators[generatorKey];

    console.log(`\n${"=".repeat(50)}`);
    console.log(`📦 ${generator.name} (${generatorKey})`);
    console.log(`${"=".repeat(50)}\n`);

    if (!generator.enabled) {
      console.log(`⏭️  Skipped (disabled)`);
      if (generator.notes) {
        console.log(`   Note: ${generator.notes}`);
      }
      continue;
    }

    const success = await runGenerator(generator, generatorKey, checkOnly);
    if (!success) {
      hasErrors = true;
    }
  }

  // Summary
  console.log(`\n${"=".repeat(50)}`);
  if (hasErrors) {
    console.error("❌ Some generators failed or detected drift.");
    if (checkOnly) {
      console.log(
        "\n💡 Run 'npx tsx scripts/docs-generators/runner.ts' to update documentation"
      );
    }
    process.exit(1);
  } else {
    console.log("✅ All documentation is up to date!");
  }
}

// ============================================================================
// Generator Execution
// ============================================================================

async function runGenerator(
  generator: GeneratorConfig,
  key: string,
  checkOnly: boolean
): Promise<boolean> {
  const generatorPath = path.join(ROOT_DIR, generator.generatorScript);

  // Check if generator script exists
  if (!fs.existsSync(generatorPath)) {
    console.log(`⚠️  Generator script not found: ${generator.generatorScript}`);
    console.log(`   Create this file to enable documentation generation.`);
    return true; // Not an error, just not implemented
  }

  try {
    // Run the generator
    const args = checkOnly ? ["--check"] : [];
    const result = spawnSync("npx", ["tsx", generatorPath, ...args], {
      cwd: ROOT_DIR,
      stdio: "inherit",
      encoding: "utf-8",
    });

    return result.status === 0;
  } catch (error) {
    console.error(`❌ Error running generator for ${key}:`, error);
    return false;
  }
}

// ============================================================================
// Changed Files Detection (for CI)
// ============================================================================

function getChangedGenerators(config: Config): string[] {
  const changedGenerators: string[] = [];

  try {
    // Get changed files from git
    // This works for both PRs (comparing to base) and pushes
    const gitDiff = execSync(
      "git diff --name-only HEAD~1 HEAD 2>/dev/null || git diff --name-only HEAD",
      { cwd: ROOT_DIR, encoding: "utf-8" }
    );

    const changedFiles = gitDiff.split("\n").filter(Boolean);

    console.log(`📝 Changed files: ${changedFiles.length}`);

    for (const [key, generator] of Object.entries(config.generators)) {
      if (!generator.enabled) continue;

      // Check if any watch path matches changed files
      const watchPatterns = generator.watchPaths.map((p) =>
        new RegExp(
          p.replace(/\*\*/g, ".*").replace(/\*/g, "[^/]*").replace(/\//g, "\\/")
        )
      );

      const hasChanges = changedFiles.some((file) =>
        watchPatterns.some((pattern) => pattern.test(file))
      );

      if (hasChanges) {
        changedGenerators.push(key);
        console.log(`   📌 ${key}: changes detected`);
      }
    }
  } catch (error) {
    console.warn("⚠️  Could not detect changed files, running all generators");
    return Object.entries(config.generators)
      .filter(([_, cfg]) => cfg.enabled)
      .map(([key, _]) => key);
  }

  return changedGenerators;
}

// ============================================================================
// List Generators
// ============================================================================

function listGenerators(config: Config): void {
  console.log("Configured Documentation Generators:\n");

  const maxKeyLen = Math.max(...Object.keys(config.generators).map((k) => k.length));

  for (const [key, generator] of Object.entries(config.generators)) {
    const status = generator.enabled ? "✅" : "⏸️ ";
    const paddedKey = key.padEnd(maxKeyLen);
    console.log(`${status} ${paddedKey}  ${generator.name} (${generator.language})`);

    if (!generator.enabled && generator.notes) {
      console.log(`   ${"".padEnd(maxKeyLen)}  └─ ${generator.notes}`);
    }
  }

  console.log("\n");
  console.log("Legend:");
  console.log("  ✅ = Enabled (will run)");
  console.log("  ⏸️  = Disabled (not yet implemented)");
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error("Error:", error);
  process.exit(1);
});
