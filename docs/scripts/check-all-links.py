#!/usr/bin/env python3
"""
Comprehensive Link Checker - Checks for broken internal links across all file types.

This script scans various file types in the docs directory and validates
that all internal links point to existing files.

Supported file types:
- MDX files (.mdx)
- TypeScript/JavaScript files (.ts, .tsx, .js, .jsx)
- JSON files (.json)
- CSS files (.css)

Usage:
    python scripts/check-all-links.py [docs_dir]

Example:
    python scripts/check-all-links.py
    python scripts/check-all-links.py /path/to/docs
"""

import os
import re
import sys
from pathlib import Path
from collections import defaultdict


# File extensions to scan
SCAN_EXTENSIONS = {'.mdx', '.md', '.tsx', '.ts', '.jsx', '.js', '.json', '.css'}

# Directories to skip
SKIP_DIRS = {'node_modules', '.next', '.git', 'dist', 'build', '.turbo', '.cache'}


def find_files(base_dir: Path) -> list[Path]:
    """Find all scannable files in the directory."""
    files = []
    for root, dirs, filenames in os.walk(base_dir):
        # Skip certain directories
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]

        for filename in filenames:
            path = Path(root) / filename
            if path.suffix in SCAN_EXTENSIONS:
                files.append(path)
    return files


def extract_links_from_content(content: str, file_type: str) -> list[tuple[str, int]]:
    """Extract all internal links from content with line numbers."""
    links = []
    lines = content.split('\n')

    for line_num, line in enumerate(lines, 1):
        # Skip comments
        stripped = line.strip()
        if stripped.startswith('//') or stripped.startswith('*') or stripped.startswith('/*'):
            continue

        # Markdown links: [text](/path) or [text](/path#anchor)
        md_link_pattern = r'\[([^\]]*)\]\((/[^)#\s]+)(?:#[^)]*)?\)'
        for match in re.finditer(md_link_pattern, line):
            links.append((match.group(2), line_num))

        # JSX/HTML href attributes: href="/path" or href='/path'
        href_pattern = r'href=["\'](/[^"\'#\s]+)(?:#[^"\']*)?["\']'
        for match in re.finditer(href_pattern, line):
            links.append((match.group(1), line_num))

        # JSX/HTML src attributes: src="/path" or src='/path'
        src_pattern = r'src=["\'](/[^"\'#\s]+)["\']'
        for match in re.finditer(src_pattern, line):
            links.append((match.group(1), line_num))

        # Next.js Link component: to="/path" or to='/path'
        to_pattern = r'\bto=["\'](/[^"\'#\s]+)(?:#[^"\']*)?["\']'
        for match in re.finditer(to_pattern, line):
            links.append((match.group(1), line_num))

        # JavaScript/TypeScript string paths (common patterns)
        # e.g., path: '/cua/guide', redirect: '/docs/cua'
        js_path_pattern = r'(?:path|href|to|redirect|url|link|route):\s*["\'](/[^"\'#\s]+)["\']'
        for match in re.finditer(js_path_pattern, line, re.IGNORECASE):
            links.append((match.group(1), line_num))

        # Image markdown: ![alt](/path)
        img_md_pattern = r'!\[[^\]]*\]\((/[^)#\s]+)(?:#[^)]*)?\)'
        for match in re.finditer(img_md_pattern, line):
            links.append((match.group(1), line_num))

    return links


def normalize_link(link: str, docs_dir: Path, public_dir: Path, src_dir: Path) -> tuple[Path | None, str]:
    """Convert a link to a file path. Returns (path, type)."""
    # Skip external links
    if link.startswith(('http://', 'https://', 'mailto:', 'tel:', '#', 'data:')):
        return None, 'external'

    # Remove leading slash
    clean_link = link.lstrip('/')

    # Skip Next.js internal routes
    if clean_link.startswith('_next/'):
        return None, 'internal'

    # Check if it's a public asset (images, etc.)
    if clean_link.startswith(('img/', 'images/', 'favicon', 'icons/')):
        return public_dir / clean_link, 'public'

    # Check if it's a docs path
    if clean_link.startswith(('docs/', 'cua/', 'cuabench/')):
        # Remove 'docs/' prefix if present
        if clean_link.startswith('docs/'):
            clean_link = clean_link[5:]
        docs_content_dir = docs_dir / 'content' / 'docs'
        return docs_content_dir / clean_link, 'docs'

    # Check source directory for components, etc.
    if clean_link.startswith(('components/', 'lib/', 'app/', 'src/')):
        return src_dir / clean_link, 'src'

    # Default: check in public directory first, then docs
    public_path = public_dir / clean_link
    if public_path.exists() or (public_path.parent.exists() and any(public_path.parent.glob(f"{public_path.name}*"))):
        return public_path, 'public'

    # Check docs content
    docs_content_dir = docs_dir / 'content' / 'docs'
    return docs_content_dir / clean_link, 'docs'


def check_file_exists(file_path: Path, link_type: str) -> bool:
    """Check if a file exists (with various extensions and index files)."""
    # Direct match
    if file_path.exists():
        return True

    # For docs, try various MDX/MD patterns
    if link_type == 'docs':
        # Try with .mdx extension
        mdx_path = file_path.with_suffix('.mdx')
        if mdx_path.exists():
            return True

        # Try with .md extension
        md_path = file_path.with_suffix('.md')
        if md_path.exists():
            return True

        # Try as directory with index.mdx
        index_mdx = file_path / 'index.mdx'
        if index_mdx.exists():
            return True

        # Try as directory with index.md
        index_md = file_path / 'index.md'
        if index_md.exists():
            return True

    # For public assets, check common image extensions
    if link_type == 'public':
        for ext in ['.png', '.jpg', '.jpeg', '.gif', '.svg', '.webp', '.ico']:
            if file_path.with_suffix(ext).exists():
                return True

    # For source files, try various extensions
    if link_type == 'src':
        for ext in ['.tsx', '.ts', '.jsx', '.js', '.css']:
            if file_path.with_suffix(ext).exists():
                return True
        # Try index files
        for ext in ['.tsx', '.ts', '.jsx', '.js']:
            index_file = file_path / f'index{ext}'
            if index_file.exists():
                return True

    return False


def check_links(docs_dir: Path) -> dict[str, list[tuple[str, int, str, str]]]:
    """Check all links in all files and return broken ones."""
    broken_links = defaultdict(list)

    public_dir = docs_dir / 'public'
    src_dir = docs_dir / 'src'
    content_dir = docs_dir / 'content'

    # Find all files to scan
    all_files = find_files(docs_dir)

    print(f"\n🔍 Scanning {len(all_files)} files in {docs_dir}\n")
    print("-" * 60)

    # Track unique broken links to avoid duplicates
    seen_broken = set()

    for file_path in all_files:
        try:
            content = file_path.read_text(encoding='utf-8')
        except Exception as e:
            print(f"⚠️  Error reading {file_path}: {e}")
            continue

        file_type = file_path.suffix
        links = extract_links_from_content(content, file_type)

        for link, line_num in links:
            # Skip external and internal Next.js links
            if link.startswith(('http://', 'https://', 'mailto:', 'tel:', '#', 'data:')):
                continue

            target_path, link_type = normalize_link(link, docs_dir, public_dir, src_dir)
            if target_path is None:
                continue

            if not check_file_exists(target_path, link_type):
                # Create unique key to avoid duplicates
                key = (str(file_path), link, line_num)
                if key not in seen_broken:
                    seen_broken.add(key)
                    rel_file = file_path.relative_to(docs_dir)
                    broken_links[str(rel_file)].append((link, line_num, str(target_path), link_type))

    return broken_links


def main():
    # Determine docs directory
    if len(sys.argv) > 1:
        docs_dir = Path(sys.argv[1])
    else:
        # Default: look for docs directory relative to script location
        script_dir = Path(__file__).parent
        docs_dir = script_dir.parent

    if not docs_dir.exists():
        print(f"❌ Directory not found: {docs_dir}")
        sys.exit(1)

    broken_links = check_links(docs_dir)

    # Summary
    print("\n" + "=" * 60)

    total_broken = sum(len(links) for links in broken_links.values())
    files_with_broken = len(broken_links)

    print(f"\n📊 Summary:")
    print(f"   Files with broken links: {files_with_broken}")
    print(f"   Total broken links: {total_broken}")

    if broken_links:
        print(f"\n❌ Broken Links:\n")

        # Group by link type
        by_type = defaultdict(list)
        for file_path, links in sorted(broken_links.items()):
            for link, line_num, target_path, link_type in links:
                by_type[link_type].append((file_path, link, line_num, target_path))

        for link_type, items in sorted(by_type.items()):
            print(f"\n   📁 {link_type.upper()} links ({len(items)}):\n")
            for file_path, link, line_num, target_path in items:
                print(f"      {file_path}:{line_num}")
                print(f"         Link: {link}")
                print(f"         Expected: {target_path}")
                print()

        # Suggest fixes
        print("\n💡 Common issues to check:")
        print("   - Links with /docs/ prefix should NOT have /docs/ (basePath handles it)")
        print("   - Internal doc links should start with /cua/ or /cuabench/")
        print("   - Image paths should start with /img/ and exist in public/img/")
        print("   - Check if target files were renamed or moved")
        print()

        return 1
    else:
        print("\n✅ All internal links are valid!\n")
        return 0


if __name__ == '__main__':
    sys.exit(main())
