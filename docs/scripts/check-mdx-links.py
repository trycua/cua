#!/usr/bin/env python3
"""
MDX Link Checker - Checks for broken internal links in MDX files.

This script scans MDX files in the docs/content/docs directory and validates
that all internal links point to existing files.

Usage:
    python scripts/check-mdx-links.py [docs_dir]

Example:
    python scripts/check-mdx-links.py
    python scripts/check-mdx-links.py /path/to/docs/content/docs
"""

import os
import re
import sys
from pathlib import Path
from collections import defaultdict


def find_mdx_files(docs_dir: Path) -> list[Path]:
    """Find all MDX files in the docs directory."""
    return list(docs_dir.rglob("*.mdx"))


def extract_links(content: str) -> list[tuple[str, int]]:
    """Extract all internal links from MDX content with line numbers."""
    links = []

    # Markdown links: [text](/path) or [text](/path#anchor)
    md_link_pattern = r'\[([^\]]*)\]\((/[^)#\s]+)(?:#[^)]*)?\)'

    # JSX href attributes: href="/path" or href='/path'
    jsx_href_pattern = r'href=["\'](/[^"\'#\s]+)(?:#[^"\']*)?["\']'

    lines = content.split('\n')
    for line_num, line in enumerate(lines, 1):
        # Find markdown links
        for match in re.finditer(md_link_pattern, line):
            links.append((match.group(2), line_num))

        # Find JSX href attributes
        for match in re.finditer(jsx_href_pattern, line):
            links.append((match.group(1), line_num))

    return links


def normalize_link_to_file_path(link: str, docs_dir: Path, public_dir: Path) -> tuple[Path | None, str]:
    """Convert an internal link to a file path. Returns (path, type)."""
    # Skip external links and anchors
    if link.startswith(('http://', 'https://', 'mailto:', 'tel:', '#')):
        return None, 'external'

    # Remove leading slash
    clean_link = link.lstrip('/')

    # Check if it's a public asset (images, etc.)
    # With basePath: '/docs', image paths are /docs/img/... which maps to public/img/
    if clean_link.startswith('docs/img/'):
        return public_dir / clean_link[5:], 'public'  # Remove 'docs/' prefix
    if clean_link.startswith('docs/favicon'):
        return public_dir / clean_link[5:], 'public'  # Remove 'docs/' prefix
    if clean_link.startswith(('img/', '_next/', 'favicon')):
        return public_dir / clean_link, 'public'

    # Skip links that go outside docs (like /get-started which doesn't exist)
    if not clean_link.startswith(('cua/', 'cuabench/')):
        # These are likely old/invalid paths
        return docs_dir / clean_link, 'docs'

    # Build the file path
    file_path = docs_dir / clean_link

    return file_path, 'docs'


def check_file_exists(file_path: Path) -> bool:
    """Check if a file exists (with or without .mdx extension, or as index.mdx)."""
    # Direct match
    if file_path.exists():
        return True

    # Try with .mdx extension
    mdx_path = file_path.with_suffix('.mdx')
    if mdx_path.exists():
        return True

    # Try as directory with index.mdx
    index_path = file_path / 'index.mdx'
    if index_path.exists():
        return True

    # Try parent directory with filename.mdx (for paths like /cua/guide)
    parent_index = file_path.parent / (file_path.name + '.mdx')
    if parent_index.exists():
        return True

    return False


def check_links(docs_dir: Path, public_dir: Path) -> dict[str, list[tuple[str, int, str]]]:
    """Check all links in MDX files and return broken ones."""
    broken_links = defaultdict(list)

    mdx_files = find_mdx_files(docs_dir)

    print(f"\n🔍 Scanning {len(mdx_files)} MDX files in {docs_dir}\n")
    print("-" * 60)

    for mdx_file in mdx_files:
        try:
            content = mdx_file.read_text(encoding='utf-8')
        except Exception as e:
            print(f"⚠️  Error reading {mdx_file}: {e}")
            continue

        links = extract_links(content)

        for link, line_num in links:
            # Skip external links
            if link.startswith(('http://', 'https://', 'mailto:', 'tel:')):
                continue

            # Skip anchor-only links
            if link.startswith('#'):
                continue

            file_path, link_type = normalize_link_to_file_path(link, docs_dir, public_dir)
            if file_path is None:
                continue

            if not check_file_exists(file_path):
                rel_file = mdx_file.relative_to(docs_dir)
                broken_links[str(rel_file)].append((link, line_num, str(file_path)))

    return broken_links


def main():
    # Determine docs directory
    if len(sys.argv) > 1:
        docs_dir = Path(sys.argv[1])
    else:
        # Default: look for docs/content/docs relative to script location
        script_dir = Path(__file__).parent
        docs_dir = script_dir.parent / 'content' / 'docs'

    # Public directory for static assets
    public_dir = docs_dir.parent.parent / 'public'

    if not docs_dir.exists():
        print(f"❌ Directory not found: {docs_dir}")
        sys.exit(1)

    broken_links = check_links(docs_dir, public_dir)

    # Summary
    print("\n" + "=" * 60)

    total_broken = sum(len(links) for links in broken_links.values())
    files_with_broken = len(broken_links)

    print(f"\n📊 Summary:")
    print(f"   Files with broken links: {files_with_broken}")
    print(f"   Total broken links: {total_broken}")

    if broken_links:
        print(f"\n❌ Broken Links:\n")

        for file_path, links in sorted(broken_links.items()):
            print(f"   📄 {file_path}")
            for link, line_num, target_path in links:
                print(f"      Line {line_num}: {link}")
                print(f"         → Expected: {target_path}")
            print()

        # Suggest fixes for common patterns
        print("\n💡 Common issues to check:")
        print("   - Links starting with /docs/ should NOT have /docs/ prefix")
        print("   - Internal links should start with /cua/ or /cuabench/")
        print("   - Check if the target file was renamed or moved")
        print()

        return 1
    else:
        print("\n✅ All internal links are valid!\n")
        return 0


if __name__ == '__main__':
    sys.exit(main())
