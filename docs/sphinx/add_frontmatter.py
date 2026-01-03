#!/usr/bin/env python3
"""
Post-process Sphinx-generated markdown files to add Fumadocs-compatible frontmatter.
"""
import os
import re
import sys
from pathlib import Path


def extract_title_from_markdown(content: str) -> str:
    """Extract the first H1 heading as the title."""
    # Look for # Title pattern
    match = re.search(r"^#\s+(.+)$", content, re.MULTILINE)
    if match:
        return match.group(1).strip()
    # Fallback to filename or default
    return "API Reference"


def fix_code_blocks(content: str) -> str:
    """Replace pycon with python in code blocks (Shiki doesn't support pycon)."""
    # Replace ```pycon with ```python
    content = re.sub(r"```pycon\b", "```python", content)
    # Also handle indented code blocks (though less common)
    return content


def fix_nested_links(content: str) -> str:
    """Remove links from inside code blocks and headings to prevent nested <a> tags."""

    # Pattern to match code blocks with links inside them
    # This matches backtick code spans that contain markdown links
    def replace_link_in_code(match):
        code_content = match.group(1)
        # Remove markdown links [text](#anchor) and replace with just text
        code_content = re.sub(r"\[([^\]]+)\]\([^\)]+\)", r"\1", code_content)
        return f"`{code_content}`"

    # Match inline code blocks that contain links
    # Pattern: `...[...](#...)...`
    content = re.sub(r"`([^`]*\[[^\]]+\]\([^\)]+\)[^`]*)`", replace_link_in_code, content)

    # Also handle code blocks that might have links in type annotations
    # Pattern: `Type`[[`Link`](#anchor)]
    content = re.sub(r"`([^`]+)`\[\[`([^`]+)`\]\([^\)]+\)\]", r"`\1[\2]`", content)

    # Remove links from headings (they become TOC links, which would nest)
    # Pattern: #### heading *: [`Link`](#anchor)*
    def replace_link_in_heading(match):
        heading_prefix = match.group(1)  # The heading markers (####, etc.)
        heading_text = match.group(2)  # Text before the link
        link_text = match.group(3)  # Link text
        heading_suffix = match.group(4) if match.group(4) else ""  # Text after link
        return f"{heading_prefix}{heading_text}{link_text}{heading_suffix}"

    # Match headings with links: #### text *: [`Link`](#anchor)*
    content = re.sub(
        r"^(#{1,6}\s+)(.*?)\s*\*\*?:\s*\[`([^`]+)`\]\([^\)]+\)(\*)?",
        replace_link_in_heading,
        content,
        flags=re.MULTILINE,
    )

    # Also handle simpler cases: #### text [`Link`](#anchor)
    content = re.sub(
        r"^(#{1,6}\s+.*?)\[`([^`]+)`\]\([^\)]+\)", r"\1\2", content, flags=re.MULTILINE
    )

    return content


def add_frontmatter_to_file(file_path: Path) -> None:
    """Add frontmatter to a markdown file and fix code block languages."""
    with open(file_path, "r", encoding="utf-8") as f:
        content = f.read()

    # Fix code blocks first
    content = fix_code_blocks(content)
    # Fix nested links in code blocks
    content = fix_nested_links(content)

    # Check if frontmatter already exists
    if content.startswith("---"):
        # Extract existing frontmatter and body
        frontmatter_match = re.match(r"^---\n(.*?)\n---\n\n?(.*)$", content, re.DOTALL)
        if frontmatter_match:
            existing_frontmatter = frontmatter_match.group(1)
            body = frontmatter_match.group(2)
            # Check if title exists
            if "title:" not in existing_frontmatter:
                # Extract title and add it
                title = extract_title_from_markdown(body)
                frontmatter = f"{existing_frontmatter}\ntitle: {title}\n"
                new_content = f"---\n{frontmatter}---\n\n{body}"
            else:
                # Frontmatter already has title, just use fixed content
                new_content = content
        else:
            # Malformed frontmatter, try to fix
            title = extract_title_from_markdown(content)
            body = re.sub(r"^---\n.*?\n---\n\n?", "", content, flags=re.DOTALL)
            new_content = f"---\ntitle: {title}\n---\n\n{body}"
    else:
        # No frontmatter, add it
        title = extract_title_from_markdown(content)
        frontmatter = f"---\ntitle: {title}\n---\n\n"
        new_content = frontmatter + content

    with open(file_path, "w", encoding="utf-8") as f:
        f.write(new_content)

    print(f"Processed {file_path}")


def process_directory(directory: Path) -> None:
    """Process all markdown files in a directory."""
    for md_file in directory.rglob("*.md"):
        try:
            add_frontmatter_to_file(md_file)
        except Exception as e:
            print(f"Error processing {md_file}: {e}", file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: add_frontmatter.py <output_directory>")
        sys.exit(1)

    output_dir = Path(sys.argv[1])
    if not output_dir.exists():
        print(f"Error: Directory {output_dir} does not exist", file=sys.stderr)
        sys.exit(1)

    process_directory(output_dir)
