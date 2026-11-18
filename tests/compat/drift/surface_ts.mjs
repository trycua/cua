#!/usr/bin/env node
// The public surface of an installed npm SDK as JSON, from its TypeScript
// declarations (the exports of the package's "types" entry, with member
// signatures). Needs `typescript` next to the package.
//
//   node surface_ts.mjs <dir containing node_modules> <package> > fixture.json
import { createRequire } from "node:module";
import path from "node:path";
import fs from "node:fs";

const [dir, pkg] = process.argv.slice(2);
if (!dir || !pkg) { console.error("usage: surface_ts.mjs DIR PACKAGE"); process.exit(2); }
const require = createRequire(path.join(path.resolve(dir), "x.js"));
const ts = require("typescript");
const pj = JSON.parse(fs.readFileSync(path.join(path.resolve(dir), "node_modules", pkg, "package.json"), "utf8"));
let entry = pj.types || pj.typings;
if (!entry && pj.exports && pj.exports["."]) {
  const e = pj.exports["."];
  entry = e.types || (e.import && e.import.types) || (e.require && e.require.types);
}
if (!entry) { console.error("no types entry in " + pkg); process.exit(1); }
const file = path.join(path.resolve(dir), "node_modules", pkg, entry);
// typeRoots is explicit: TypeScript otherwise looks for @types under the
// current directory, and the surface would depend on where this is run from.
const program = ts.createProgram([file], { allowJs: false, skipLibCheck: true, noEmit: true, moduleResolution: ts.ModuleResolutionKind.Bundler ?? ts.ModuleResolutionKind.NodeJs, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, typeRoots: [path.join(path.resolve(dir), "node_modules", "@types")] });
const checker = program.getTypeChecker();
const sf = program.getSourceFile(file);
const modSym = checker.getSymbolAtLocation(sf);
const out = { package: pkg, version: pj.version, symbols: {} };
const flags = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope | ts.TypeFormatFlags.WriteArrowStyleSignature;
const T = (t) => checker.typeToString(t, undefined, flags).replace(/import\("[^"]*"\)\./g, "");
const sigs = (type, decl) => checker.getSignaturesOfType(type, ts.SignatureKind.Call).map((s) => T(checker.getTypeOfSymbolAtLocation(s.getDeclaration().symbol ?? type.symbol, s.getDeclaration())) && checker.signatureToString(s, undefined, flags).replace(/import\("[^"]*"\)\./g, ""));
function members(type, decl) {
  const m = {};
  for (const p of checker.getPropertiesOfType(type)) {
    if (p.name.startsWith("_") || p.name.startsWith("#")) continue;
    const pd = p.valueDeclaration ?? p.declarations?.[0];
    if (pd && ts.getCombinedModifierFlags(pd) & (ts.ModifierFlags.Private | ts.ModifierFlags.Protected)) continue;
    const pt = checker.getTypeOfSymbolAtLocation(p, pd ?? decl);
    const calls = checker.getSignaturesOfType(pt, ts.SignatureKind.Call);
    m[p.name] = calls.length ? calls.map((s) => checker.signatureToString(s, undefined, flags).replace(/import\("[^"]*"\)\./g, "")).sort() : T(pt);
    if (p.flags & ts.SymbolFlags.Optional) m[p.name] = ["?", m[p.name]];
  }
  return m;
}
for (let sym of checker.getExportsOfModule(modSym)) {
  const name = sym.name;
  const target = sym.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(sym) : sym;
  const decl = target.declarations?.[0];
  if (!decl) { out.symbols[name] = { kind: "unknown" }; continue; }
  const entry = { };
  if (target.flags & ts.SymbolFlags.Class) {
    entry.kind = "class";
    const inst = checker.getDeclaredTypeOfSymbol(target);
    entry.members = members(inst, decl);
    const ctor = checker.getTypeOfSymbolAtLocation(target, decl);
    entry.construct = checker.getSignaturesOfType(ctor, ts.SignatureKind.Construct).map((s) => checker.signatureToString(s, undefined, flags).replace(/import\("[^"]*"\)\./g, ""));
    entry.static = members(ctor, decl);
    delete entry.static.prototype;
  } else if (target.flags & ts.SymbolFlags.Enum) {
    entry.kind = "enum";
    entry.members = Object.fromEntries(checker.getPropertiesOfType(checker.getTypeOfSymbolAtLocation(target, decl)).map((p) => [p.name, T(checker.getTypeOfSymbolAtLocation(p, decl))]));
  } else if (target.flags & ts.SymbolFlags.Interface) {
    entry.kind = "interface";
    entry.members = members(checker.getDeclaredTypeOfSymbol(target), decl);
  } else if (target.flags & ts.SymbolFlags.TypeAlias) {
    entry.kind = "type";
    entry.type = T(checker.getDeclaredTypeOfSymbol(target));
  } else if (target.flags & ts.SymbolFlags.Function) {
    entry.kind = "function";
    entry.sigs = checker.getSignaturesOfType(checker.getTypeOfSymbolAtLocation(target, decl), ts.SignatureKind.Call).map((s) => checker.signatureToString(s, undefined, flags).replace(/import\("[^"]*"\)\./g, "")).sort();
  } else {
    entry.kind = "value";
    entry.type = T(checker.getTypeOfSymbolAtLocation(target, decl));
  }
  out.symbols[name] = entry;
}
const sort = (o) => Array.isArray(o) ? o.map(sort) : o && typeof o === "object" ? Object.fromEntries(Object.keys(o).sort().map((k) => [k, sort(o[k])])) : o;
process.stdout.write(JSON.stringify(sort(out), null, 1) + "\n");
