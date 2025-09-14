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
    
    def search_google_shopping(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Google Shopping for products"""
        products = []
        try:
            search_url = f"https://www.google.com/search?tbm=shop&q={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            
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
        """Search Canadian Tire for products"""
        products = []
        try:
            search_url = f"https://www.canadiantire.ca/en/search-results.html?q={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            
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
