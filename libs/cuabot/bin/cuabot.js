#!/usr/bin/env node
import { fileURLToPath, pathToFileURL } from "url";
import { dirname, join } from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Import and run the compiled cuabot module
const cuabotPath = join(__dirname, "..", "dist", "cuabot.js");
const importPath = process.platform === "win32" ? pathToFileURL(cuabotPath).href : cuabotPath;
import(importPath);
