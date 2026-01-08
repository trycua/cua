"""
Comprehensive crawler for cua.ai/docs using crawl4ai
Recursively crawls all documentation pages and saves content to JSON files
"""

import asyncio
import json
import re
from pathlib import Path
from urllib.parse import urljoin, urlparse
from crawl4ai import AsyncWebCrawler, BrowserConfig, CrawlerRunConfig

# Configuration
BASE_URL = "https://cua.ai"
DOCS_URL = f"{BASE_URL}/docs"
OUTPUT_DIR = Path(__file__).parent.parent / "crawled_data"
MAX_CONCURRENT = 5  # Limit concurrent requests to be polite
DELAY_BETWEEN_REQUESTS = 0.5  # seconds


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
        path = parsed.path.rstrip('/')
        if not path:
            path = ''
        return f"{parsed.scheme}://{parsed.netloc}{path}"

    def is_valid_url(self, url: str) -> bool:
        """Check if URL should be crawled (only /docs pages)"""
        parsed = urlparse(url)

        # Only crawl cua.ai pages
        if parsed.netloc and parsed.netloc not in ['cua.ai', 'www.cua.ai']:
            return False

        # Only crawl /docs paths
        if not parsed.path.startswith('/docs'):
            return False

        # Skip non-page resources
        skip_extensions = ['.pdf', '.png', '.jpg', '.jpeg', '.gif', '.svg', '.css', '.js', '.ico', '.woff', '.woff2', '.ttf', '.zip', '.tar', '.gz']
        if any(parsed.path.lower().endswith(ext) for ext in skip_extensions):
            return False

        # Skip external links and anchors
        if url.startswith('#') or url.startswith('mailto:') or url.startswith('javascript:'):
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
            if href.startswith('/'):
                full_url = urljoin(BASE_URL, href)
            elif href.startswith('http'):
                full_url = href
            elif not href.startswith('#') and not href.startswith('mailto:'):
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
        path = parsed.path.replace('/docs/', '').strip('/')
        parts = path.split('/') if path else []

        return {
            "path": path,
            "category": parts[0] if parts else "root",
            "subcategory": parts[1] if len(parts) > 1 else None,
            "page": parts[-1] if parts else "index",
            "depth": len(parts),
        }

    async def crawl_page(self, crawler: AsyncWebCrawler, url: str) -> dict | None:
        """Crawl a single page"""
        async with self.semaphore:
            try:
                print(f"Crawling: {url}")

                config = CrawlerRunConfig(
                    word_count_threshold=10,
                    exclude_external_links=True,
                )

                result = await crawler.arun(url=url, config=config)

                if result.success:
                    # Extract new links from the page
                    new_links = self.extract_links(result.html, url)
                    for link in new_links:
                        if link not in self.visited_urls and link not in self.to_visit:
                            self.to_visit.add(link)

                    path_info = self.extract_path_info(url)

                    page_data = {
                        "url": url,
                        "title": result.metadata.get("title", "") if result.metadata else "",
                        "description": result.metadata.get("description", "") if result.metadata else "",
                        "markdown": result.markdown,
                        "path_info": path_info,
                        "links_found": list(new_links),
                    }

                    # Save individual page
                    self.save_page(url, page_data)

                    await asyncio.sleep(DELAY_BETWEEN_REQUESTS)
                    return page_data
                else:
                    print(f"Failed to crawl {url}: {result.error_message}")
                    self.failed_urls.add(url)
                    return None

            except Exception as e:
                print(f"Error crawling {url}: {e}")
                self.failed_urls.add(url)
                return None

    def save_page(self, url: str, data: dict):
        """Save page data to a JSON file"""
        # Create filename from URL path
        parsed = urlparse(url)
        path = parsed.path.strip('/') or 'index'
        filename = path.replace('/', '_') + '.json'

        filepath = OUTPUT_DIR / filename
        with open(filepath, 'w', encoding='utf-8') as f:
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
            if self.is_valid_url(normalized) or url.endswith('llms.txt'):
                self.to_visit.add(normalized)

        browser_config = BrowserConfig(
            headless=True,
            verbose=False,
        )

        async with AsyncWebCrawler(config=browser_config) as crawler:
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
                tasks = [self.crawl_page(crawler, url) for url in batch]
                results = await asyncio.gather(*tasks)

                # Collect successful results
                for result in results:
                    if result:
                        self.all_data.append(result)

                print(f"Progress: {len(self.visited_urls)} crawled, {len(self.to_visit)} remaining")

        # Save summary
        summary = {
            "total_pages": len(self.all_data),
            "failed_urls": list(self.failed_urls),
            "all_urls": list(self.visited_urls),
            "categories": self._get_categories(),
        }

        with open(OUTPUT_DIR / "_summary.json", 'w', encoding='utf-8') as f:
            json.dump(summary, f, indent=2)

        # Save all data in one file too
        with open(OUTPUT_DIR / "_all_pages.json", 'w', encoding='utf-8') as f:
            json.dump(self.all_data, f, indent=2, ensure_ascii=False)

        print(f"\nCrawl complete!")
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
