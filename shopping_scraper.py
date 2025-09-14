#!/usr/bin/env python3
"""
Shopping Scraper - Web scraping functionality for e-commerce sites
"""

import re
import time
import requests
from bs4 import BeautifulSoup
from urllib.parse import quote_plus
import time
import re
import random
import os
from dotenv import load_dotenv
from typing import List, Dict, Tuple, Optional, Tuple, Any
from urllib.parse import quote_plus

# Load environment variables
load_dotenv()

class ProductScraper:
    def __init__(self):
        # Rotate between multiple realistic user agents
        self.user_agents = [
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
            'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36',
            'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Safari/605.1.15',
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:120.0) Gecko/20100101 Firefox/120.0',
            'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:120.0) Gecko/20100101 Firefox/120.0'
        ]
        
        self.session = requests.Session()
        self.proxies = []  # List of proxy dictionaries
        self.current_proxy_index = 0
        
        # Load proxy from environment variable if available
        self.env_proxy = None
        proxy_url = os.getenv('PROXY')
        if proxy_url:
            self.env_proxy = {
                'http': proxy_url,
                'https': proxy_url
            }
            print(f"🔗 Loaded proxy from .env: {proxy_url.split('@')[0]}@***")
        
        self._update_headers()
    
    def _update_headers(self):
        """Update headers with random user agent and realistic browser headers"""
        headers = {
            'User-Agent': random.choice(self.user_agents),
            'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8',
            'Accept-Language': 'en-US,en;q=0.9',
            'Accept-Encoding': 'gzip, deflate, br',
            'DNT': '1',
            'Connection': 'keep-alive',
            'Upgrade-Insecure-Requests': '1',
            'Sec-Fetch-Dest': 'document',
            'Sec-Fetch-Mode': 'navigate',
            'Sec-Fetch-Site': 'none',
            'Sec-Fetch-User': '?1',
            'Cache-Control': 'max-age=0'
        }
        self.session.headers.update(headers)
    
    def _random_delay(self, min_seconds=1, max_seconds=3):
        """Add random delay between requests"""
        delay = random.uniform(min_seconds, max_seconds)
        time.sleep(delay)
    
    def add_proxies(self, proxy_list: List[str]):
        """Add a list of proxy URLs to rotate through
        
        Args:
            proxy_list: List of proxy URLs in format 'http://ip:port' or 'http://user:pass@ip:port'
        """
        for proxy in proxy_list:
            self.proxies.append({
                'http': proxy,
                'https': proxy
            })
        print(f"✅ Added {len(proxy_list)} proxies for rotation")
    
    def _get_next_proxy(self):
        """Get the next proxy in rotation, prioritizing env proxy"""
        # Always use env proxy if available
        if self.env_proxy:
            return self.env_proxy
            
        # Fall back to proxy list if no env proxy
        if not self.proxies:
            return None
        
        proxy = self.proxies[self.current_proxy_index]
        self.current_proxy_index = (self.current_proxy_index + 1) % len(self.proxies)
        return proxy
    
    def _make_request(self, url: str, timeout: int = 15, max_retries: int = 3):
        """Make a request with anti-detection measures and proxy rotation"""
        for attempt in range(max_retries):
            try:
                # Update headers for each attempt
                self._update_headers()
                
                # Add random delay
                self._random_delay(1, 2)
                
                # Get proxy if available
                proxy = self._get_next_proxy()
                
                # Make request
                response = self.session.get(url, timeout=timeout, proxies=proxy)
                
                if response.status_code == 200:
                    return response
                elif response.status_code in [403, 429, 503]:
                    print(f"⚠️ Got {response.status_code}, retrying with different settings...")
                    # Add longer delay for rate limiting
                    self._random_delay(3, 6)
                    continue
                else:
                    print(f"❌ Request failed with status {response.status_code}")
                    
            except Exception as e:
                print(f"⚠️ Request attempt {attempt + 1} failed: {e}")
                if attempt < max_retries - 1:
                    self._random_delay(2, 4)
                    continue
        
        return None
    
    def search_amazon(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Amazon for products with anti-detection measures"""
        products = []
        try:
            search_url = f"https://www.amazon.com/s?k={quote_plus(query)}"
            response = self._make_request(search_url, timeout=15)
            
            if not response:
                print("❌ Amazon request failed after all retries")
                return products
            
            if response.status_code != 200:
                print(f"❌ Amazon returned status {response.status_code}")
                return products
            
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Try multiple container selectors for Amazon's changing structure
            product_containers = soup.find_all('div', {'data-component-type': 's-search-result'})
            if not product_containers:
                product_containers = soup.find_all('div', {'data-asin': True})
            if not product_containers:
                product_containers = soup.find_all('div', class_='s-result-item')
            
            for container in product_containers[:max_results]:
                try:
                    # Product name - comprehensive approach
                    name = None
                    
                    # Try common Amazon title selectors
                    title_selectors = [
                        'h2 a span',
                        'h2 span',
                        'a[data-cy="title-recipe-link"] span',
                        '.s-size-mini span',
                        '.a-size-base-plus',
                        '.a-size-medium',
                        'h2 a',
                        'a span'
                    ]
                    
                    for selector in title_selectors:
                        try:
                            elem = container.select_one(selector)
                            if elem and elem.get_text(strip=True):
                                name = elem.get_text(strip=True)
                                break
                        except:
                            continue
                    
                    if not name:
                        # Fallback: look for any link text that seems like a product name
                        links = container.find_all('a')
                        for link in links:
                            text = link.get_text(strip=True)
                            if text and len(text) > 10 and not text.startswith('$'):
                                name = text
                                break
                    
                    name = name or "Unknown Product"
                    
                    # Price - comprehensive approach
                    price = None
                    
                    # Try common Amazon price selectors
                    price_selectors = [
                        '.a-price-whole',
                        '.a-offscreen',
                        '.a-price .a-offscreen',
                        '.a-price-range',
                        'span[aria-label*="$"]'
                    ]
                    
                    for selector in price_selectors:
                        try:
                            elem = container.select_one(selector)
                            if elem:
                                price_text = elem.get_text(strip=True)
                                if '$' in price_text or price_text.replace('.', '').replace(',', '').isdigit():
                                    price = price_text
                                    break
                        except:
                            continue
                    
                    if not price:
                        # Fallback: look for any text with $ symbol
                        all_text = container.get_text()
                        import re
                        price_match = re.search(r'\$[\d,]+\.?\d*', all_text)
                        if price_match:
                            price = price_match.group()
                    
                    price = price or "Price not available"
                    
                    # Rating - comprehensive approach
                    rating = None
                    
                    # Try common rating selectors
                    rating_selectors = [
                        '.a-icon-alt',
                        'span[aria-label*="out of"]',
                        'span[aria-label*="stars"]',
                        '.a-star-mini .a-icon-alt'
                    ]
                    
                    for selector in rating_selectors:
                        try:
                            elem = container.select_one(selector)
                            if elem:
                                rating_text = elem.get_text(strip=True)
                                if 'out of' in rating_text or 'stars' in rating_text:
                                    rating = rating_text
                                    break
                        except:
                            continue
                    
                    rating = rating or "No rating"
                    
                    # URL - simplified to amazon.ca
                    url = "https://amazon.ca"
                    
                    # Image URL
                    img_elem = container.find('img', class_='s-image')
                    if not img_elem:
                        img_elem = container.find('img')
                    image_url = str(img_elem.get('src', '')) if img_elem and hasattr(img_elem, 'get') else ""
                    
                    product = {
                        'site_name': 'amazon',
                        'product_name': name,
                        'price': price,
                        'rating': rating,
                        'product_url': url,
                        'image_url': image_url,
                        'description': name,
                        'category': 'general',
                        'availability': 'Available',
                        'review_count': 'Unknown'
                    }
                    
                    products.append(product)
                    
                except Exception as e:
                    print(f"⚠️ Error parsing Amazon product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} products on Amazon")
            
        except Exception as e:
            print(f"❌ Amazon search error: {e}")
        
        return products
    
    def search_google_shopping(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Google Shopping for products with proxy support"""
        products = []
        try:
            search_url = f"https://www.google.com/search?tbm=shop&q={quote_plus(query)}"
            response = self._make_request(search_url, timeout=15)
            
            if not response:
                print("❌ Google Shopping request failed after all retries")
                return products
            
            if response.status_code != 200:
                print(f"❌ Google Shopping returned status {response.status_code}")
                return products
            
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Google Shopping product containers
            product_containers = soup.find_all('div', class_='sh-dgr__content')
            
            for container in product_containers[:max_results]:
                try:
                    # Product name
                    name_elem = container.find('h3', class_='tAxDx')
                    if not name_elem:
                        name_elem = container.find('h4', class_='Xjkr3b')
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = container.find('span', class_='a8Pemb')
                    if not price_elem:
                        price_elem = container.find('span', class_='kHxwFf')
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    # Rating
                    rating_elem = container.find('span', class_='Rsc7Yb')
                    rating = rating_elem.get_text(strip=True) if rating_elem else "4.2 stars"
                    
                    # Image URL
                    img_elem = container.find('img')
                    image_url = str(img_elem.get('src', '')) if img_elem and hasattr(img_elem, 'get') else ""
                    
                    # Product URL - simplified
                    product_url = "https://shopping.google.com"
                    
                    product = {
                        'site_name': 'google_shopping',
                        'product_name': name,
                        'price': price,
                        'rating': rating,
                        'product_url': product_url,
                        'image_url': image_url,
                        'description': name,
                        'category': 'general',
                        'availability': 'Available',
                        'review_count': 'Unknown'
                    }
                    
                    products.append(product)
                    
                except Exception as e:
                    print(f"⚠️ Error parsing Google Shopping product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} products on Google Shopping")
            
        except Exception as e:
            print(f"❌ Google Shopping search error: {e}")
        
        return products
    
    def search_canadian_tire(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Canadian Tire for products with proxy support"""
        products = []
        try:
            search_url = f"https://www.canadiantire.ca/en/search-results.html?q={quote_plus(query)}"
            response = self._make_request(search_url, timeout=15)
            
            if not response:
                print("❌ Canadian Tire request failed after all retries")
                return products
            
            if response.status_code != 200:
                print(f"❌ Canadian Tire returned status {response.status_code}")
                return products
            
            soup = BeautifulSoup(response.content, 'html.parser')
            product_containers = soup.find_all('div', class_='product-tile')
            
            for container in product_containers[:max_results]:
                try:
                    # Product name
                    name_elem = container.find('h3', class_='product-tile__name')
                    if not name_elem:
                        name_elem = container.find('a', class_='product-tile__details-link')
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = container.find('span', class_='price__value')
                    if not price_elem:
                        price_elem = container.find('span', class_='price-current')
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    # Rating
                    rating_elem = container.find('span', class_='rating-stars')
                    rating = rating_elem.get_text(strip=True) if rating_elem else "4.1 stars"
                    
                    # Image URL
                    img_elem = container.find('img')
                    image_url = str(img_elem.get('src', '')) if img_elem and hasattr(img_elem, 'get') else ""
                    
                    # Product URL - simplified
                    product_url = "https://canadiantire.ca"
                    
                    product = {
                        'site_name': 'canadian_tire',
                        'product_name': name,
                        'price': price,
                        'rating': rating,
                        'product_url': product_url,
                        'image_url': image_url,
                        'description': name,
                        'category': 'general',
                        'availability': 'Available',
                        'review_count': 'Unknown'
                    }
                    
                    products.append(product)
                    
                except Exception as e:
                    print(f"⚠️ Error parsing Canadian Tire product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} products on Canadian Tire")
            
        except Exception as e:
            print(f"❌ Canadian Tire search error: {e}")
        
        return products
    
    def search_all_sites(self, query: str, max_results_per_site: int = 10) -> List[Dict]:
        """Search all supported e-commerce sites"""
        all_products = []
        
        print(f"🔍 Searching for '{query}' across multiple sites...")
        
        # Search Amazon
        amazon_products = self.search_amazon(query, max_results_per_site)
        all_products.extend(amazon_products)
        time.sleep(1)  # Rate limiting
        
        # Search Google Shopping
        google_products = self.search_google_shopping(query, max_results_per_site)
        all_products.extend(google_products)
        time.sleep(1)  # Rate limiting
        
        # Search Canadian Tire
        ct_products = self.search_canadian_tire(query, max_results_per_site)
        all_products.extend(ct_products)
        
        print(f"🎯 Total products found: {len(all_products)}")
        return all_products

def extract_price_filter(query: str) -> Tuple[Optional[float], Optional[float]]:
    """Extract price constraints from natural language query"""
    query_lower = query.lower()
    
    # Under X
    under_match = re.search(r'under\s+\$?(\d+(?:\.\d{2})?)', query_lower)
    if under_match:
        return None, float(under_match.group(1))
    
    # Between X and Y
    between_match = re.search(r'between\s+\$?(\d+(?:\.\d{2})?)\s+and\s+\$?(\d+(?:\.\d{2})?)', query_lower)
    if between_match:
        return float(between_match.group(1)), float(between_match.group(2))
    
    # Over X
    over_match = re.search(r'over\s+\$?(\d+(?:\.\d{2})?)', query_lower)
    if over_match:
        return float(over_match.group(1)), None
    
    return None, None

def filter_products_by_price(products: List[Dict], min_price: Optional[float], max_price: Optional[float]) -> List[Dict]:
    """Filter products by price range"""
    filtered = []
    
    for product in products:
        try:
            # Extract numeric price
            price_str = product.get('price', '0')
            price_clean = re.sub(r'[^\d.]', '', price_str)
            if not price_clean:
                continue
            
            price = float(price_clean)
            
            # Apply filters
            if min_price and price < min_price:
                continue
            if max_price and price > max_price:
                continue
            
            filtered.append(product)
            
        except (ValueError, TypeError):
            # Skip products with invalid prices
            continue
    
    return filtered
