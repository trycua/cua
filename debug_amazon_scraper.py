#!/usr/bin/env python3
"""
Debug script to analyze Amazon's HTML structure and fix parsing
"""

import requests
from bs4 import BeautifulSoup
from urllib.parse import quote_plus
import os
from dotenv import load_dotenv

load_dotenv()

def debug_amazon_structure():
    """Debug Amazon's HTML structure to understand parsing issues"""
    
    # Setup session with proxy
    session = requests.Session()
    proxy_url = os.getenv('PROXY')
    proxies = None
    if proxy_url:
        proxies = {'http': proxy_url, 'https': proxy_url}
        print(f"🔗 Using proxy: {proxy_url.split('@')[0]}@***")
    
    # Headers
    headers = {
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
        'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
        'Accept-Language': 'en-US,en;q=0.9',
        'Accept-Encoding': 'gzip, deflate, br',
        'DNT': '1',
        'Connection': 'keep-alive',
        'Upgrade-Insecure-Requests': '1'
    }
    session.headers.update(headers)
    
    query = "teddy bears"
    search_url = f"https://www.amazon.com/s?k={quote_plus(query)}"
    
    print(f"🔍 Fetching: {search_url}")
    
    try:
        response = session.get(search_url, timeout=15, proxies=proxies)
        print(f"📊 Status: {response.status_code}")
        
        if response.status_code == 200:
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Save HTML for inspection
            with open('amazon_debug.html', 'w', encoding='utf-8') as f:
                f.write(soup.prettify())
            print("💾 Saved HTML to amazon_debug.html")
            
            # Try different container selectors
            print("\n🔍 Analyzing product containers:")
            
            selectors_to_try = [
                ('div[data-component-type="s-search-result"]', 'data-component-type'),
                ('div[data-asin]', 'data-asin'),
                ('div.s-result-item', 'class s-result-item'),
                ('div[data-index]', 'data-index'),
                ('[data-cy="title-recipe-link"]', 'title-recipe-link')
            ]
            
            for selector, desc in selectors_to_try:
                containers = soup.select(selector)
                print(f"  {desc}: {len(containers)} found")
                
                if containers:
                    print(f"    First container preview:")
                    first = containers[0]
                    
                    # Look for titles
                    title_elements = first.find_all(['h1', 'h2', 'h3', 'span', 'a'])
                    for elem in title_elements[:5]:
                        text = elem.get_text(strip=True)
                        if text and len(text) > 10:
                            print(f"      Title candidate: {text[:60]}...")
                    
                    # Look for prices
                    price_elements = first.find_all(string=lambda text: text and '$' in str(text))
                    for price in price_elements[:3]:
                        if price.strip():
                            print(f"      Price candidate: {price.strip()}")
                    
                    print()
            
            # Check if we're getting blocked
            page_text = soup.get_text().lower()
            if 'robot' in page_text or 'captcha' in page_text or 'blocked' in page_text:
                print("⚠️  Possible blocking detected in page content")
                
        else:
            print(f"❌ Failed with status {response.status_code}")
            print(f"Response headers: {dict(response.headers)}")
            
    except Exception as e:
        print(f"❌ Error: {e}")

if __name__ == "__main__":
    debug_amazon_structure()
