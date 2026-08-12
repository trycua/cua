import { readFile, writeFile } from "node:fs/promises"

const generatedClientPath = process.argv[2] ?? new URL("../src/generated/cyclops-cs-backend.ts", import.meta.url)
const original = await readFile(generatedClientPath, "utf8")
const oldImplementation = `  private customFetch = (...fetchParams: Parameters<typeof fetch>) =>
    fetch(...fetchParams);`
const newImplementation = `  private customFetch: typeof fetch = (...fetchParams) => {
    const runtimeFetch = globalThis.fetch;
    if (typeof runtimeFetch !== "function") {
      throw new Error(
        "Fetch API is unavailable. @trycua/cyclops requires Node.js 18+ or an injected customFetch implementation.",
      );
    }
    return runtimeFetch(...fetchParams);
  };`

if (!original.includes(oldImplementation)) {
  throw new Error("Generated HttpClient fetch implementation did not match the expected template")
}

const oldBillingSummaryCard = "  card?: BillingCardSummary | null;";
const newBillingSummaryCard = "  card: BillingCardSummary | null;";

if (!original.includes(oldBillingSummaryCard)) {
  throw new Error("Generated BillingSummary card declaration did not match the expected template");
}

const patched = original
  .replace(oldImplementation, newImplementation)
  .replace(oldBillingSummaryCard, newBillingSummaryCard);

await writeFile(generatedClientPath, patched)
