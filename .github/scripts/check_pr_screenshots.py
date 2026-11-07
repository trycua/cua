#!/usr/bin/env python3
"""Flag a UI pull request whose description has no screenshots.

Reads the description from PR_BODY. It passes when the description (HTML
comments removed, so the template's own instructions do not count) either

* has at least two images and mentions both "desktop" and "mobile", or
* has a line `Screenshots: not needed - <reason>` with a real reason.

Exit 0 on pass, 1 on a missing one. Run by ci-pr-screenshots.yml.
"""
from __future__ import annotations

import os
import re
import sys

IMAGE = re.compile(
    r"!\[[^\]]*\]\([^)\s]+|<img\b[^>]*\bsrc=|github\.com/user-attachments/|private-user-images\.githubusercontent\.com",
    re.IGNORECASE,
)
WAIVER = re.compile(r"^\s*screenshots:\s*not needed\s*[-:]\s*(\S.{6,})$", re.IGNORECASE | re.MULTILINE)


def strip_comments(body: str) -> str:
    return re.sub(r"<!--.*?-->", "", body, flags=re.DOTALL)


def evaluate(body: str) -> tuple[bool, str]:
    text = strip_comments(body or "")
    if WAIVER.search(text):
        return True, "Waived: the description says why no screenshots are needed."
    images = len(IMAGE.findall(text))
    lower = text.lower()
    missing = []
    if images < 2:
        missing.append(f"at least two images (found {images})")
    if "desktop" not in lower:
        missing.append('the word "desktop" next to the desktop image')
    if "mobile" not in lower:
        missing.append('the word "mobile" next to the mobile image')
    if missing:
        return False, "Missing " + ", ".join(missing) + "."
    return True, f"Found {images} images with desktop and mobile labels."


def main() -> int:
    ok, why = evaluate(os.environ.get("PR_BODY", ""))
    if ok:
        print(why)
        return 0
    print(
        "::error title=UI pull request without screenshots::"
        f"{why} Add before/after screenshots at desktop (1440x900) and mobile (390x844) "
        "to the description, with demo data only, or write "
        "`Screenshots: not needed - <reason>`. See .github/CONTRIBUTING.md#show-ui-changes."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
