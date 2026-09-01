const major = Number.parseInt(process.versions.node.split('.')[0] ?? '', 10);

if (!Number.isFinite(major) || major < 22 || major >= 26) {
  throw new Error(
    `Electron packaging requires Node 22 through 25. Current runtime: ${process.version}.`,
  );
}
