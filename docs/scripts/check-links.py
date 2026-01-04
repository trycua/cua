#!/usr/bin/env python3
"""
Link checker for the docs website.
Crawls all pages starting from the base URL and reports broken links.

Usage:
    python scripts/check-links.py [base_url]

Example:
    python scripts/check-links.py http://localhost:8090
"""

import sys
import urllib.request
import urllib.parse
import urllib.error
from html.parser import HTMLParser
from collections import deque
import ssl

# Ignore SSL certificate errors for local development
ssl._create_default_https_context = ssl._create_unverified_context


class LinkExtractor(HTMLParser):
    """Extract all href and src attributes from HTML."""

    def __init__(self):
        super().__init__()
        self.links = []

    def handle_starttag(self, tag, attrs):
        attrs_dict = dict(attrs)
        if tag == 'a' and 'href' in attrs_dict:
            self.links.append(attrs_dict['href'])
        elif tag in ('img', 'script') and 'src' in attrs_dict:
            self.links.append(attrs_dict['src'])
        elif tag == 'link' and 'href' in attrs_dict:
            self.links.append(attrs_dict['href'])


def normalize_url(base_url, link):
    """Normalize a link relative to a base URL."""
    # Skip mailto, tel, javascript, and anchor-only links
    if link.startswith(('mailto:', 'tel:', 'javascript:', '#')):
        return None

    # Parse and join URLs
    parsed = urllib.parse.urljoin(base_url, link)

    # Remove fragment
    parsed = urllib.parse.urldefrag(parsed)[0]

    return parsed


def is_internal(url, base_domain):
    """Check if URL is internal to the base domain."""
    parsed = urllib.parse.urlparse(url)
    return parsed.netloc == base_domain or parsed.netloc == ''


def fetch_url(url, timeout=10):
    """Fetch a URL and return (status_code, content, content_type)."""
    try:
        req = urllib.request.Request(
            url,
            headers={'User-Agent': 'LinkChecker/1.0'}
        )
        response = urllib.request.urlopen(req, timeout=timeout)
        content_type = response.headers.get('Content-Type', '')
        content = response.read() if 'text/html' in content_type else b''
        return response.status, content, content_type
    except urllib.error.HTTPError as e:
        return e.code, b'', ''
    except urllib.error.URLError as e:
        return None, b'', str(e.reason)
    except Exception as e:
        return None, b'', str(e)


def check_links(base_url):
    """Crawl the website and check all links."""
    parsed_base = urllib.parse.urlparse(base_url)
    base_domain = parsed_base.netloc

    visited = set()
    to_visit = deque([base_url])
    broken_links = []
    checked_external = {}

    print(f"\n🔍 Checking links starting from {base_url}\n")
    print("-" * 60)

    while to_visit:
        current_url = to_visit.popleft()

        if current_url in visited:
            continue

        visited.add(current_url)

        # Only crawl internal pages
        if not is_internal(current_url, base_domain):
            continue

        status, content, content_type = fetch_url(current_url)

        if status is None:
            print(f"❌ ERROR: {current_url}")
            broken_links.append((current_url, 'Connection error', None))
            continue
        elif status >= 400:
            print(f"❌ {status}: {current_url}")
            broken_links.append((current_url, status, None))
            continue
        else:
            print(f"✓ {status}: {current_url}")

        # Parse HTML and extract links
        if 'text/html' in content_type and content:
            try:
                parser = LinkExtractor()
                parser.feed(content.decode('utf-8', errors='ignore'))

                for link in parser.links:
                    normalized = normalize_url(current_url, link)
                    if normalized is None:
                        continue

                    if is_internal(normalized, base_domain):
                        # Queue internal links for crawling
                        if normalized not in visited:
                            to_visit.append(normalized)
                    else:
                        # Check external links once
                        if normalized not in checked_external:
                            ext_status, _, _ = fetch_url(normalized, timeout=5)
                            checked_external[normalized] = ext_status
                            if ext_status is None or ext_status >= 400:
                                print(f"  ⚠️  External broken: {normalized} (from {current_url})")
                                broken_links.append((normalized, ext_status, current_url))
            except Exception as e:
                print(f"  ⚠️  Parse error on {current_url}: {e}")

    # Summary
    print("\n" + "=" * 60)
    print(f"\n📊 Summary:")
    print(f"   Pages checked: {len(visited)}")
    print(f"   Broken links: {len(broken_links)}")

    if broken_links:
        print(f"\n❌ Broken Links:\n")
        for url, status, source in broken_links:
            status_str = str(status) if status else 'Error'
            if source:
                print(f"   [{status_str}] {url}")
                print(f"         Found on: {source}")
            else:
                print(f"   [{status_str}] {url}")
        print()
        return 1
    else:
        print("\n✅ All links are valid!\n")
        return 0


if __name__ == '__main__':
    base_url = sys.argv[1] if len(sys.argv) > 1 else 'http://localhost:8090'

    # Ensure base URL has /docs prefix
    if not base_url.endswith('/docs'):
        base_url = base_url.rstrip('/') + '/docs'

    sys.exit(check_links(base_url))
