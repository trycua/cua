#!/usr/bin/env python3
"""
Check for broken internal links in all .md and .mdx files across the entire repository.
This script validates:
- Relative links to other markdown files
- Relative links to directories
- Absolute paths within the repo
- Image references
"""

import os
import re
import sys
from pathlib import Path
from collections import defaultdict
from urllib.parse import urlparse

# Directories to skip
SKIP_DIRS = {'node_modules', '.next', '.git', 'dist', 'build', '.turbo', '.cache', '__pycache__', '.venv', 'venv'}

# Link patterns
MD_LINK_PATTERN = re.compile(r'\[([^\]]*)\]\(([^)]+)\)')
HTML_HREF_PATTERN = re.compile(r'(?:href|src)=["\']([^"\']+)["\']')


def find_md_files(root_dir: Path) -> list[Path]:
    """Find all .md and .mdx files in the repository."""
    md_files = []
    for dirpath, dirnames, filenames in os.walk(root_dir):
        # Skip unwanted directories
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]

        for filename in filenames:
            if filename.endswith(('.md', '.mdx')):
                md_files.append(Path(dirpath) / filename)

    return md_files


def extract_links(file_path: Path) -> list[tuple[int, str]]:
    """Extract all links from a markdown file. Returns list of (line_number, link)."""
    links = []
    try:
        content = file_path.read_text(encoding='utf-8')
    except Exception as e:
        print(f"Warning: Could not read {file_path}: {e}")
        return links

    for line_num, line in enumerate(content.split('\n'), 1):
        # Find markdown links
        for match in MD_LINK_PATTERN.finditer(line):
            link = match.group(2).split('#')[0].strip()  # Remove anchors
            if link:
                links.append((line_num, link))

        # Find HTML href/src attributes
        for match in HTML_HREF_PATTERN.finditer(line):
            link = match.group(1).split('#')[0].strip()
            if link:
                links.append((line_num, link))

    return links


def is_external_link(link: str) -> bool:
    """Check if a link is external."""
    return link.startswith(('http://', 'https://', 'mailto:', 'tel:', 'ftp://', '//'))


def is_anchor_only(link: str) -> bool:
    """Check if a link is an anchor-only link."""
    return link.startswith('#')


def resolve_link(link: str, source_file: Path, repo_root: Path) -> tuple[Path | None, str]:
    """
    Resolve a link to an absolute path.
    Returns (resolved_path, link_type).
    """
    if is_external_link(link) or is_anchor_only(link):
        return None, 'external'

    # Handle absolute paths (starting with /)
    if link.startswith('/'):
        # Could be repo-relative or docs-relative
        clean_link = link.lstrip('/')

        # Check if it's a docs path
        if clean_link.startswith(('docs/', 'cua/', 'cuabench/')):
            # This is likely a docs link - check in docs/content/docs
            docs_content = repo_root / 'docs' / 'content' / 'docs'
            if clean_link.startswith('docs/'):
                return docs_content / clean_link[5:], 'docs'
            return docs_content / clean_link, 'docs'

        # Check if it exists from repo root
        return repo_root / clean_link, 'repo'

    # Handle relative paths
    source_dir = source_file.parent
    resolved = (source_dir / link).resolve()

    return resolved, 'relative'


def check_path_exists(path: Path) -> bool:
    """Check if a path exists, considering it could be a file or directory."""
    if path.exists():
        return True

    # Try with common extensions
    for ext in ['.md', '.mdx', '.tsx', '.ts', '.jsx', '.js', '.py', '.json']:
        if path.with_suffix(ext).exists():
            return True

    # Try as directory with index file
    if path.is_dir():
        for index in ['index.md', 'index.mdx', 'README.md']:
            if (path / index).exists():
                return True

    # Check if it's a file reference without extension
    parent = path.parent
    name = path.name
    if parent.exists():
        # Check for any file with this base name
        matches = list(parent.glob(f"{name}.*"))
        if matches:
            return True

    return False


def main():
    # Determine repo root (go up from script location)
    script_dir = Path(__file__).resolve().parent
    repo_root = script_dir.parent  # Assuming script is in /scripts

    # Allow override via command line
    if len(sys.argv) > 1:
        repo_root = Path(sys.argv[1]).resolve()

    print(f"Scanning for .md/.mdx files in: {repo_root}\n")

    md_files = find_md_files(repo_root)
    print(f"Found {len(md_files)} markdown files\n")

    broken_links = defaultdict(list)
    total_links = 0
    external_links = 0

    for md_file in md_files:
        links = extract_links(md_file)

        for line_num, link in links:
            total_links += 1

            if is_external_link(link) or is_anchor_only(link):
                external_links += 1
                continue

            resolved_path, link_type = resolve_link(link, md_file, repo_root)

            if resolved_path is None:
                continue

            if not check_path_exists(resolved_path):
                rel_file = md_file.relative_to(repo_root)
                broken_links[str(rel_file)].append({
                    'line': line_num,
                    'link': link,
                    'expected': str(resolved_path),
                    'type': link_type
                })

    # Print results
    print("=" * 70)
    print(f"\nSummary:")
    print(f"  Total links checked: {total_links}")
    print(f"  External/anchor links (skipped): {external_links}")
    print(f"  Internal links checked: {total_links - external_links}")
    print(f"  Broken links found: {sum(len(v) for v in broken_links.values())}")
    print(f"  Files with broken links: {len(broken_links)}")

    if broken_links:
        print("\n" + "=" * 70)
        print("\nBroken Links:\n")

        for file_path, links in sorted(broken_links.items()):
            print(f"  {file_path}")
            for info in links:
                print(f"    Line {info['line']}: {info['link']}")
                print(f"      -> Expected: {info['expected']}")
            print()

        sys.exit(1)
    else:
        print("\n All internal links are valid!")
        sys.exit(0)


if __name__ == '__main__':
    main()
