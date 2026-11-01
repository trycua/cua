// The public surface of the `e2b` npm package, as JSON: every exported
// class, interface, function, enum and type alias, and the members of each
// with their signatures. The weekly drift job diffs the latest package with
// the committed snapshot, so a new method is noticed the week it ships.
//
//   node surface_ts.mjs <path to node_modules/e2b> [typescript module dir]  > surface-ts-X.json
//   node surface_ts.mjs --diff fixtures/surface-ts-X.json <path to e2b>
//
// The package is read as data (its .d.ts); nothing from it is executed.
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import path from "node:path";

const argv = process.argv.slice(2);
let diffAgainst = null;
if (argv[0] === "--diff") { diffAgainst = argv[1]; argv.splice(0, 2); }
const pkgDir = path.resolve(argv[0]);
const require = createRequire(import.meta.url);
const ts = require(argv[1] ? path.resolve(argv[1]) : "typescript");

const pkg = JSON.parse(readFileSync(path.join(pkgDir, "package.json"), "utf8"));
const entry = path.join(pkgDir, pkg.types || "dist/index.d.ts");
const program = ts.createProgram([entry], { noEmit: true, skipLibCheck: true, target: ts.ScriptTarget.ES2022, moduleResolution: ts.ModuleResolutionKind.Bundler, module: ts.ModuleKind.ESNext, types: [] });
const checker = program.getTypeChecker();
const sf = program.getSourceFile(entry);
const modSym = checker.getSymbolAtLocation(sf);
const fmt = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope;

function sigs(type, decl) {
  const calls = checker.getSignaturesOfType(type, ts.SignatureKind.Call);
  if (calls.length) return calls.map((s) => checker.signatureToString(s, decl, fmt)).join(" | ");
  return checker.typeToString(type, decl, fmt);
}
function members(type, decl) {
  const out = {};
  for (const p of checker.getPropertiesOfType(type)) {
    if (p.name.startsWith("_") || p.name.startsWith("#")) continue;
    const d = p.valueDeclaration || p.declarations?.[0];
    if (d && ts.getCombinedModifierFlags(d) & ts.ModifierFlags.Private) continue;
    if (d && ts.getCombinedModifierFlags(d) & ts.ModifierFlags.Protected) continue;
    out[p.name] = d ? sigs(checker.getTypeOfSymbolAtLocation(p, d), d) : "?";
  }
  return out;
}

const surface = {};
for (const exp of checker.getExportsOfModule(modSym)) {
  let sym = exp;
  if (sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
  const decl = sym.declarations?.[0];
  if (!decl) { surface[exp.name] = { kind: "?" }; continue; }
  const entry = {};
  if (sym.flags & ts.SymbolFlags.Class) {
    entry.kind = "class";
    const inst = checker.getDeclaredTypeOfSymbol(sym);
    entry.members = members(inst, decl);
    const stat = checker.getTypeOfSymbolAtLocation(sym, decl);
    entry.static = members(stat, decl);
    const ctor = checker.getSignaturesOfType(stat, ts.SignatureKind.Construct);
    entry.ctor = ctor.map((s) => checker.signatureToString(s, decl, fmt)).join(" | ");
    entry.extends = (inst.getBaseTypes?.() || []).map((b) => checker.typeToString(b));
  } else if (sym.flags & ts.SymbolFlags.Interface) {
    entry.kind = "interface";
    entry.members = members(checker.getDeclaredTypeOfSymbol(sym), decl);
  } else if (sym.flags & ts.SymbolFlags.Enum) {
    entry.kind = "enum";
    entry.members = Object.fromEntries(checker.getPropertiesOfType(checker.getTypeOfSymbolAtLocation(sym, decl)).map((p) => [p.name, "member"]));
  } else if (sym.flags & ts.SymbolFlags.Function) {
    entry.kind = "function";
    entry.signature = sigs(checker.getTypeOfSymbolAtLocation(sym, decl), decl);
  } else if (sym.flags & ts.SymbolFlags.TypeAlias) {
    entry.kind = "type";
    entry.type = checker.typeToString(checker.getDeclaredTypeOfSymbol(sym), decl, fmt | ts.TypeFormatFlags.InTypeAlias);
  } else {
    entry.kind = "value";
    entry.type = checker.typeToString(checker.getTypeOfSymbolAtLocation(sym, decl), decl, fmt);
  }
  surface[exp.name] = entry;
}
const out = { version: pkg.version, exports: surface };

const clip = (v) => { const t = JSON.stringify(v); return t && t.length > 200 ? t.slice(0, 200) + `... (${t.length} chars)` : t; };
// For long type texts, the words that were added or removed.
function wordDiff(a, b) {
  if (typeof a !== "string" || typeof b !== "string" || a.length < 200) return "";
  const wa = new Set(a.split(/[\s{};:,|()<>\[\]]+/)), wb = new Set(b.split(/[\s{};:,|()<>\[\]]+/));
  const add = [...wb].filter((w) => !wa.has(w)), del = [...wa].filter((w) => !wb.has(w));
  return `  [+${add.slice(0, 20).join(" ")}] [-${del.slice(0, 20).join(" ")}]`;
}

function diff(a, b) {
  const d = [];
  for (const k of new Set([...Object.keys(a.exports), ...Object.keys(b.exports)])) {
    const x = a.exports[k], y = b.exports[k];
    if (!x) { d.push(`+ export ${k} (${y.kind})`); continue; }
    if (!y) { d.push(`- export ${k} (${x.kind})`); continue; }
    for (const part of ["members", "static"]) {
      const m = x[part] || {}, n = y[part] || {};
      for (const f of new Set([...Object.keys(m), ...Object.keys(n)])) {
        if (m[f] !== n[f]) d.push(`${k}.${part === "static" ? "static " : ""}${f}: ${clip(m[f])} -> ${clip(n[f])}`);
      }
    }
    for (const f of ["ctor", "signature", "type", "extends"]) {
      if (JSON.stringify(x[f]) !== JSON.stringify(y[f])) d.push(`${k} ${f}: ${clip(x[f])} -> ${clip(y[f])}${wordDiff(x[f], y[f])}`);
    }
  }
  return d;
}

if (diffAgainst) {
  const old = JSON.parse(readFileSync(diffAgainst, "utf8"));
  const d = diff(old, out);
  if (d.length) { console.log(`${d.length} change(s) in the e2b npm surface since ${diffAgainst}:`); d.forEach((x) => console.log("  " + x)); process.exit(1); }
  console.log(`npm surface unchanged since ${diffAgainst}`);
} else {
  const sorted = (o) => Array.isArray(o) ? o.map(sorted) : o && typeof o === "object" ? Object.fromEntries(Object.keys(o).sort().map((k) => [k, sorted(o[k])])) : o;
  console.log(JSON.stringify(sorted(out), null, 1));
}
