#!/usr/bin/env node
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Import and run the compiled cuabot module
import(join(__dirname, "..", "dist", "cuabot.js"));
