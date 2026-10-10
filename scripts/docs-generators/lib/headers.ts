/**
 * Curated headers for generated pages.
 *
 * A generated page may open with a short hand-written section (context the
 * source cannot carry, such as a cross-cutting contract). It lives in
 * `scripts/docs-generators/headers/<product>/<page>.md`, is owned by the
 * generator (the page is still regenerated, never hand-edited) and is part of
 * the generator's watch paths.
 */

import * as fs from 'fs';
import * as path from 'path';

export const HEADERS_DIR = path.join(__dirname, '..', 'headers');

/** The header for `<product>/<page>`, or undefined when there is none. */
export function readHeader(product: string, page: string): string | undefined {
  const file = path.join(HEADERS_DIR, product, `${page}.md`);
  return fs.existsSync(file) ? fs.readFileSync(file, 'utf-8').trim() : undefined;
}
