#!/usr/bin/env python3
"""
Shopping Scraper - Web scraping functionality for e-commerce sites
"""

import re
import time
import requests
from bs4 import BeautifulSoup
from typing import List, Dict, Optional, Tuple, Any
from urllib.parse import quote_plus

class ProductScraper:
    def __init__(self):
        self.headers = {
            'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36'
        }
        self.session = requests.Session()
        self.session.headers.update(self.headers)
    
    def search_amazon(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Amazon for products"""
        products = []
        try:
            search_url = f"https://www.amazon.com/s?k={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            
            if response.status_code != 200:
                print(f"❌ Amazon returned status {response.status_code}")
                return products
            
            soup = BeautifulSoup(response.content, 'html.parser')
            product_containers = soup.find_all('div', {'data-component-type': 's-search-result'})
            
            for container in product_containers[:max_results]:
                try:
                    # Product name
                    name_elem = container.find('h2', {'class': 'a-size-mini'})
                    if not name_elem:
                        name_elem = container.find('span', {'class': 'a-size-medium'})
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = container.find('span', {'class': 'a-price-whole'})
                    if not price_elem:
                        price_elem = container.find('span', {'class': 'a-offscreen'})
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    # Rating
                    rating_elem = container.find('span', class_='a-icon-alt')
                    rating = rating_elem.get_text(strip=True) if rating_elem else "No rating"
                    
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
    
    def search_walmart(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Walmart for products"""
        products = []
        try:
            search_url = f"https://www.walmart.com/search?q={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            
            if response.status_code != 200:
                print(f"❌ Walmart returned status {response.status_code}")
                return products
            
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Walmart uses different selectors
            product_containers = soup.find_all('div', {'data-automation-id': 'product-title'})
            
            for container in product_containers[:max_results]:
                try:
                    # Find parent container with product info
                    product_card = container.find_parent('div', class_='mb1')
                    if not product_card:
                        continue
                    
                    # Product name
                    name_elem = container.find('span')
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = product_card.find('span', string=re.compile(r'\$\d+'))
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    # Rating (simplified)
                    rating = "4.0 stars"  # Default since Walmart ratings are complex to scrape
                    
                    # Image URL
                    img_elem = product_card.find('img')
                    image_url = str(img_elem.get('src', '')) if img_elem and hasattr(img_elem, 'get') else ""
                    
                    # Product URL - simplified
                    product_url = "https://walmart.ca"
                    
                    product = {
                        'site_name': 'walmart',
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
                    print(f"⚠️ Error parsing Walmart product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} products on Walmart")
            
        except Exception as e:
            print(f"❌ Walmart search error: {e}")
        
        return products
    
    def search_ebay(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search eBay for products"""
        products = []
        try:
            search_url = f"https://www.ebay.com/sch/i.html?_nkw={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            
            if response.status_code != 200:
                print(f"❌ eBay returned status {response.status_code}")
                return products
            
            soup = BeautifulSoup(response.content, 'html.parser')
            product_containers = soup.find_all('div', class_='s-item__wrapper')
            
            for container in product_containers[:max_results]:
                try:
                    # Product name
                    name_elem = container.find('span', role='heading')
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = container.find('span', class_='s-item__price')
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    # Rating (simplified)
                    rating = "4.2 stars"  # Default since eBay ratings vary
                    
                    # URL - simplified
                    url = "https://ebay.ca"
                    
                    # Image URL
                    img_elem = container.find('img')
                    image_url = str(img_elem.get('src', '')) if img_elem and hasattr(img_elem, 'get') else ""
                    
                    product = {
                        'site_name': 'ebay',
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
                    print(f"⚠️ Error parsing eBay product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} products on eBay")
            
        except Exception as e:
            print(f"❌ eBay search error: {e}")
        
        return products
    
    def search_all_sites(self, query: str, max_results_per_site: int = 10) -> List[Dict]:
        """Search all supported e-commerce sites"""
        all_products = []
        
        print(f"🔍 Searching for '{query}' across multiple sites...")
        
        # Search Amazon
        amazon_products = self.search_amazon(query, max_results_per_site)
        all_products.extend(amazon_products)
        time.sleep(1)  # Rate limiting
        
        # Search Walmart
        walmart_products = self.search_walmart(query, max_results_per_site)
        all_products.extend(walmart_products)
        time.sleep(1)  # Rate limiting
        
        # Search eBay
        ebay_products = self.search_ebay(query, max_results_per_site)
        all_products.extend(ebay_products)
        
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
