/**
 * Shared HTTP utilities for CUA SDK requests.
 */

export const CUA_CLIENT_VERSION_HEADER = 'X-Cua-Client-Version';

/**
 * Build a version header dict for CUA SDK requests.
 *
 * @param packageName - short package name, e.g. `'agent'`, `'computer'`
 * @param version     - semver string injected at build time
 * @returns header record; empty when version is falsy, so always safe to spread.
 */
export function cuaVersionHeaders(
  packageName: string,
  version: string,
): Record<string, string> {
  if (!version) return {};
  return { [CUA_CLIENT_VERSION_HEADER]: `${packageName}:${version}` };
}
