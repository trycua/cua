"""
Comprehensive crawler for cua.ai/docs using Playwright.
Recursively crawls all documentation pages and saves content to JSON files.
"""

import asyncio
import html
from html.parser import HTMLParser
import json
import re
from pathlib import Path
from urllib.parse import urljoin, urlparse

from playwright.async_api import Browser, async_playwright

# Configuration
BASE_URL = "https://cua.ai"
DOCS_URL = f"{BASE_URL}/docs"
OUTPUT_DIR = Path(__file__).parent.parent / "crawled_data"
MAX_CONCURRENT = 5  # Limit concurrent requests to be polite
DELAY_BETWEEN_REQUESTS = 0.5  # seconds


class HTMLToMarkdown(HTMLParser):
    """Small dependency-free HTML-to-Markdown converter for crawled docs pages.

    Extraction is scoped to the page's main content container (``<article>``,
    falling back to ``<main>``) and site chrome (``nav``/``aside``/``footer``) is
    dropped, so the crawled corpus is the documentation body rather than the
    navigation tree that repeats identically on every page.
    """

    block_tags = {
        "blockquote",
        "br",
        "div",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "li",
        "main",
        "ol",
        "p",
        "pre",
        "section",
        "table",
        "tr",
        "ul",
    }
    # Content of these tags is dropped entirely: non-text assets and the site
    # chrome (sidebar/nav tree, "on this page" aside, footer) that is identical
    # on every page and would otherwise dominate the embedded corpus.
    skip_tags = {"script", "style", "svg", "nav", "aside", "footer"}

    def __init__(self, scope_tag: str | None = None) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.skip_depth = 0
        self.in_pre = False
        # When set, only emit text while inside this container; None = emit all.
        self.scope_tag = scope_tag
        self.scope_depth = 0

    @property
    def _capturing(self) -> bool:
        return self.scope_tag is None or self.scope_depth > 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in self.skip_tags:
            self.skip_depth += 1
            return
        if tag == self.scope_tag:
            self.scope_depth += 1
        if self.skip_depth or not self._capturing:
            return
        if tag in self.block_tags:
            self.parts.append("\n")
        if tag == "li":
            self.parts.append("- ")
        elif tag == "pre":
            self.in_pre = True
            self.parts.append("\n```\n")
        elif tag == "code" and not self.in_pre:
            self.parts.append("`")

    def handle_endtag(self, tag: str) -> None:
        if tag in self.skip_tags and self.skip_depth:
            self.skip_depth -= 1
            return
        if self.skip_depth:
            return
        if self._capturing:
            if tag == "pre":
                self.in_pre = False
                self.parts.append("\n```\n")
            elif tag == "code" and not self.in_pre:
                self.parts.append("`")
            if tag in self.block_tags:
                self.parts.append("\n")
        if tag == self.scope_tag and self.scope_depth:
            self.scope_depth -= 1

    def handle_data(self, data: str) -> None:
        if self.skip_depth or not self._capturing:
            return
        text = data if self.in_pre else re.sub(r"\s+", " ", data)
        if text.strip():
            self.parts.append(text)

    def markdown(self) -> str:
        text = html.unescape("".join(self.parts))
        text = re.sub(r"[ \t]+\n", "\n", text)
        text = re.sub(r"\n{3,}", "\n\n", text)
        return text.strip()


def html_to_markdown(page_html: str) -> str:
    # Prefer the main content container so the navigation/sidebar chrome that
    # repeats on every page does not pollute the crawled corpus; fall back to
    # the whole document when neither container is present.
    scope_tag = None
    for tag in ("article", "main"):
        if re.search(rf"<{tag}[\s>]", page_html, re.IGNORECASE):
            scope_tag = tag
            break
    parser = HTMLToMarkdown(scope_tag)
    parser.feed(page_html)
    return parser.markdown()


def extract_metadata(page_html: str, title: str) -> dict[str, str]:
    description = ""
    match = re.search(
        r'<meta[^>]+name=["\']description["\'][^>]+content=["\']([^"\']*)["\']',
        page_html,
        re.IGNORECASE,
    )
    if match:
        description = html.unescape(match.group(1))
    return {"title": title, "description": description}


class CuaDocsCrawler:
    def __init__(self):
        self.visited_urls: set[str] = set()
        self.to_visit: set[str] = set()
        self.failed_urls: set[str] = set()
        self.all_data: list[dict] = []
        self.semaphore = asyncio.Semaphore(MAX_CONCURRENT)

    def normalize_url(self, url: str) -> str:
        """Normalize URL to avoid duplicates"""
        parsed = urlparse(url)
        # Remove trailing slashes and fragments
        path = parsed.path.rstrip("/")
        if not path:
            path = ""
        return f"{parsed.scheme}://{parsed.netloc}{path}"

    def is_valid_url(self, url: str) -> bool:
        """Check if URL should be crawled (only /docs pages)"""
        parsed = urlparse(url)

        # Only crawl cua.ai pages
        if parsed.netloc and parsed.netloc not in ["cua.ai", "www.cua.ai"]:
            return False

        # Only crawl /docs paths
        if not parsed.path.startswith("/docs"):
            return False

        # Skip non-page resources
        skip_extensions = [
            ".pdf",
            ".png",
            ".jpg",
            ".jpeg",
            ".gif",
            ".svg",
            ".css",
            ".js",
            ".ico",
            ".woff",
            ".woff2",
            ".ttf",
            ".zip",
            ".tar",
            ".gz",
        ]
        if any(parsed.path.lower().endswith(ext) for ext in skip_extensions):
            return False

        # Skip external links and anchors
        if url.startswith("#") or url.startswith("mailto:") or url.startswith("javascript:"):
            return False

        return True

    def extract_links(self, html: str, current_url: str) -> set[str]:
        """Extract all internal links from HTML content"""
        links = set()

        # Find all href attributes
        href_pattern = r'href=["\']([^"\']+)["\']'
        matches = re.findall(href_pattern, html, re.IGNORECASE)

        for href in matches:
            # Convert relative URLs to absolute
            if href.startswith("/"):
                full_url = urljoin(BASE_URL, href)
            elif href.startswith("http"):
                full_url = href
            elif not href.startswith("#") and not href.startswith("mailto:"):
                full_url = urljoin(current_url, href)
            else:
                continue

            normalized = self.normalize_url(full_url)
            if self.is_valid_url(normalized):
                links.add(normalized)

        return links

    def extract_path_info(self, url: str) -> dict:
        """Extract meaningful path information from URL"""
        parsed = urlparse(url)
        path = parsed.path.replace("/docs/", "").strip("/")
        parts = path.split("/") if path else []

        return {
            "path": path,
            "category": parts[0] if parts else "root",
            "subcategory": parts[1] if len(parts) > 1 else None,
            "page": parts[-1] if parts else "index",
            "depth": len(parts),
        }

    async def crawl_page(self, browser: Browser, url: str) -> dict | None:
        """Crawl a single page"""
        async with self.semaphore:
            page = None
            try:
                print(f"Crawling: {url}")

                page = await browser.new_page()
                response = await page.goto(url, wait_until="networkidle", timeout=30_000)
                if response is None or not response.ok:
                    status = response.status if response else "no response"
                    print(f"Failed to crawl {url}: HTTP {status}")
                    self.failed_urls.add(url)
                    return None

                page_html = await page.content()
                metadata = extract_metadata(page_html, await page.title())

                # Extract new links from the page
                new_links = self.extract_links(page_html, url)
                for link in new_links:
                    if link not in self.visited_urls and link not in self.to_visit:
                        self.to_visit.add(link)

                path_info = self.extract_path_info(url)

                page_data = {
                    "url": url,
                    "title": metadata["title"],
                    "description": metadata["description"],
                    "markdown": html_to_markdown(page_html),
                    "path_info": path_info,
                    "links_found": list(new_links),
                }

                # Save individual page
                self.save_page(url, page_data)

                await asyncio.sleep(DELAY_BETWEEN_REQUESTS)
                return page_data

            except Exception as e:
                print(f"Error crawling {url}: {e}")
                self.failed_urls.add(url)
                return None
            finally:
                if page is not None:
                    await page.close()

    def save_page(self, url: str, data: dict):
        """Save page data to a JSON file"""
        # Create filename from URL path
        parsed = urlparse(url)
        path = parsed.path.strip("/") or "index"
        filename = path.replace("/", "_") + ".json"

        filepath = OUTPUT_DIR / filename
        with open(filepath, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2, ensure_ascii=False)

    async def crawl_all(self):
        """Main crawl loop"""
        OUTPUT_DIR.mkdir(exist_ok=True)

        # Start with the docs URL and key sections based on typical CUA docs structure
        seed_urls = [
            DOCS_URL,
            f"{DOCS_URL}/cua",
            f"{DOCS_URL}/cua/guide",
            f"{DOCS_URL}/cua/guide/get-started",
            f"{DOCS_URL}/cua/reference",
            f"{DOCS_URL}/cua/reference/computer-sdk",
            f"{DOCS_URL}/cua-bench",
            f"{BASE_URL}/llms.txt",  # LLM-optimized content if available
        ]

        for url in seed_urls:
            normalized = self.normalize_url(url)
            if self.is_valid_url(normalized) or url.endswith("llms.txt"):
                self.to_visit.add(normalized)

        async with async_playwright() as playwright:
            browser = await playwright.chromium.launch(headless=True)
            try:
                while self.to_visit:
                    # Get batch of URLs to crawl
                    batch = []
                    while self.to_visit and len(batch) < MAX_CONCURRENT:
                        url = self.to_visit.pop()
                        if url not in self.visited_urls:
                            batch.append(url)
                            self.visited_urls.add(url)

                    if not batch:
                        break

                    # Crawl batch concurrently
                    tasks = [self.crawl_page(browser, url) for url in batch]
                    results = await asyncio.gather(*tasks)

                    # Collect successful results
                    for result in results:
                        if result:
                            self.all_data.append(result)

                    print(
                        f"Progress: {len(self.visited_urls)} crawled, "
                        f"{len(self.to_visit)} remaining"
                    )
            finally:
                await browser.close()

        # Save summary
        summary = {
            "total_pages": len(self.all_data),
            "failed_urls": list(self.failed_urls),
            "all_urls": list(self.visited_urls),
            "categories": self._get_categories(),
        }

        with open(OUTPUT_DIR / "_summary.json", "w", encoding="utf-8") as f:
            json.dump(summary, f, indent=2)

        # Save all data in one file too
        with open(OUTPUT_DIR / "_all_pages.json", "w", encoding="utf-8") as f:
            json.dump(self.all_data, f, indent=2, ensure_ascii=False)

        print("\nCrawl complete!")
        print(f"Total pages crawled: {len(self.all_data)}")
        print(f"Failed URLs: {len(self.failed_urls)}")
        print(f"Output saved to: {OUTPUT_DIR.absolute()}")

    def _get_categories(self) -> dict:
        """Get summary of categories crawled"""
        categories = {}
        for page in self.all_data:
            cat = page.get("path_info", {}).get("category", "unknown")
            categories[cat] = categories.get(cat, 0) + 1
        return categories


async def main():
    crawler = CuaDocsCrawler()
    await crawler.crawl_all()


if __name__ == "__main__":
    asyncio.run(main())
