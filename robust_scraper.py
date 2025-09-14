#!/usr/bin/env python3
"""
Robust Shopping Scraper - Enhanced scraper with anti-bot protection bypass
"""

import re
import time
import random
import requests
from bs4 import BeautifulSoup
from typing import List, Dict, Optional, Tuple, Any
from urllib.parse import quote_plus
import json

class RobustProductScraper:
    def __init__(self):
        # Rotate user agents to avoid detection
        self.user_agents = [
            'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
            'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Safari/605.1.15',
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/121.0'
        ]
        self.session = requests.Session()
        self.update_headers()
    
    def update_headers(self):
        """Update session headers with random user agent"""
        headers = {
            'User-Agent': random.choice(self.user_agents),
            'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8',
            'Accept-Language': 'en-US,en;q=0.5',
            'Accept-Encoding': 'gzip, deflate',
            'Connection': 'keep-alive',
            'Upgrade-Insecure-Requests': '1',
        }
        self.session.headers.update(headers)
    
    def generate_mock_products(self, query: str, site: str, count: int = 5) -> List[Dict]:
        """Generate realistic mock products when scraping fails"""
        mock_products = []
        
        # Product templates based on common queries
        templates = {
            'headphones': [
                {'name': 'Sony WH-1000XM4 Wireless Headphones', 'price': '$299.99', 'rating': '4.5 stars'},
                {'name': 'Bose QuietComfort 35 II', 'price': '$249.99', 'rating': '4.4 stars'},
                {'name': 'Apple AirPods Pro', 'price': '$199.99', 'rating': '4.3 stars'},
                {'name': 'Sennheiser HD 450BT', 'price': '$149.99', 'rating': '4.2 stars'},
                {'name': 'JBL Tune 750BTNC', 'price': '$99.99', 'rating': '4.1 stars'}
            ],
            'laptop': [
                {'name': 'MacBook Air M2', 'price': '$1199.99', 'rating': '4.8 stars'},
                {'name': 'Dell XPS 13', 'price': '$999.99', 'rating': '4.6 stars'},
                {'name': 'HP Pavilion 15', 'price': '$699.99', 'rating': '4.3 stars'},
                {'name': 'Lenovo ThinkPad E15', 'price': '$799.99', 'rating': '4.4 stars'},
                {'name': 'ASUS VivoBook 15', 'price': '$549.99', 'rating': '4.2 stars'}
            ],
            'phone': [
                {'name': 'iPhone 15 Pro', 'price': '$999.99', 'rating': '4.7 stars'},
                {'name': 'Samsung Galaxy S24', 'price': '$799.99', 'rating': '4.5 stars'},
                {'name': 'Google Pixel 8', 'price': '$699.99', 'rating': '4.4 stars'},
                {'name': 'OnePlus 12', 'price': '$799.99', 'rating': '4.3 stars'},
                {'name': 'Motorola Edge 40', 'price': '$499.99', 'rating': '4.1 stars'}
            ]
        }
        
        # Determine product category
        query_lower = query.lower()
        category = 'general'
        products_data = []
        
        if any(word in query_lower for word in ['headphone', 'earphone', 'airpod', 'audio']):
            category = 'headphones'
            products_data = templates['headphones']
        elif any(word in query_lower for word in ['laptop', 'computer', 'macbook']):
            category = 'laptop'
            products_data = templates['laptop']
        elif any(word in query_lower for word in ['phone', 'iphone', 'samsung', 'mobile']):
            category = 'phone'
            products_data = templates['phone']
        else:
            # Generic products
            products_data = [
                {'name': f'Premium {query.title()}', 'price': '$199.99', 'rating': '4.5 stars'},
                {'name': f'Best {query.title()} Pro', 'price': '$149.99', 'rating': '4.3 stars'},
                {'name': f'Quality {query.title()} Plus', 'price': '$99.99', 'rating': '4.2 stars'},
                {'name': f'Budget {query.title()}', 'price': '$49.99', 'rating': '4.0 stars'},
                {'name': f'Deluxe {query.title()} Set', 'price': '$299.99', 'rating': '4.6 stars'}
            ]
        
        for i, product_data in enumerate(products_data[:count]):
            product = {
                'site_name': site,
                'product_name': product_data['name'],
                'price': product_data['price'],
                'rating': product_data['rating'],
                'product_url': f'https://www.{site}.com/product/{i+1}',
                'description': product_data['name'],
                'category': category,
                'availability': 'Available',
                'review_count': str(random.randint(100, 5000)),
                'image_url': f'https://via.placeholder.com/300x300?text={quote_plus(product_data["name"][:20])}'
            }
            mock_products.append(product)
        
        return mock_products
    
    def search_amazon(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Amazon with enhanced anti-bot protection"""
        products = []
        try:
            self.update_headers()
            search_url = f"https://www.amazon.com/s?k={quote_plus(query)}"
            
            # Add random delay
            time.sleep(random.uniform(1, 3))
            
            response = self.session.get(search_url, timeout=15)
            
            if response.status_code != 200:
                print(f"❌ Amazon blocked request (status {response.status_code})")
                return []
            
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Try multiple selectors for Amazon products
            selectors = [
                'div[data-component-type="s-search-result"]',
                '.s-result-item',
                '[data-asin]'
            ]
            
            product_containers = []
            for selector in selectors:
                product_containers = soup.select(selector)
                if product_containers:
                    break
            
            if not product_containers:
                print("❌ No Amazon products found with selectors")
                return []
            
            for container in product_containers[:max_results]:
                try:
                    # Multiple strategies for product name
                    name = None
                    name_selectors = ['h2 a span', '.s-size-mini span', 'h2 span']
                    for sel in name_selectors:
                        elem = container.select_one(sel)
                        if elem and elem.get_text(strip=True):
                            name = elem.get_text(strip=True)
                            break
                    
                    if not name:
                        continue
                    
                    # Multiple strategies for price
                    price = "Price not available"
                    price_selectors = ['.a-price-whole', '.a-offscreen', '.a-price .a-offscreen']
                    for sel in price_selectors:
                        elem = container.select_one(sel)
                        if elem and elem.get_text(strip=True):
                            price = elem.get_text(strip=True)
                            if '$' not in price:
                                price = f"${price}"
                            break
                    
                    # Rating
                    rating_elem = container.select_one('.a-icon-alt')
                    rating = rating_elem.get_text(strip=True) if rating_elem else "4.0 stars"
                    
                    # URL
                    link_elem = container.select_one('h2 a')
                    url = f"https://www.amazon.com{link_elem['href']}" if link_elem and link_elem.get('href') else ""
                    
                    # Image
                    img_elem = container.select_one('img')
                    image_url = img_elem.get('src') if img_elem else ""
                    
                    product = {
                        'site_name': 'amazon',
                        'product_name': name,
                        'price': price,
                        'rating': rating,
                        'product_url': url,
                        'description': name,
                        'category': 'general',
                        'availability': 'Available',
                        'review_count': 'Unknown',
                        'image_url': image_url
                    }
                    
                    products.append(product)
                    
                except Exception as e:
                    print(f"⚠️ Error parsing Amazon product: {e}")
                    continue
            
            if not products:
                print("❌ No valid Amazon products parsed")
                return []
            
            print(f"✅ Found {len(products)} real products on Amazon")
            
        except Exception as e:
            print(f"❌ Amazon search error: {e}")
            return []
        
        return products
    
    def search_walmart(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Walmart with fallback to mock data"""
        try:
            self.update_headers()
            # Add Walmart-specific headers
            self.session.headers.update({
                'Referer': 'https://www.walmart.com/',
                'sec-ch-ua': '"Not_A Brand";v="8", "Chromium";v="120"'
            })
            
            search_url = f"https://www.walmart.com/search?q={quote_plus(query)}"
            time.sleep(random.uniform(1, 2))
            
            response = self.session.get(search_url, timeout=15)
            
            if response.status_code != 200:
                print(f"❌ Walmart blocked request")
                return []
            
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Try to scrape real Walmart data
            product_containers = soup.find_all('div', {'data-automation-id': 'product-title'})
            
            if not product_containers:
                print("❌ No Walmart products found")
                return []
            
            products = []
            for container in product_containers[:max_results]:
                try:
                    product_card = container.find_parent('div', class_='mb1')
                    if not product_card:
                        continue
                    
                    name_elem = container.find('span')
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    price_elem = product_card.find('span', text=re.compile(r'\$\d+'))
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    product = {
                        'site_name': 'walmart',
                        'product_name': name,
                        'price': price,
                        'rating': 'No rating',
                        'product_url': search_url,
                        'description': name,
                        'category': 'general',
                        'availability': 'Available',
                        'review_count': 'Unknown',
                        'image_url': ''
                    }
                    
                    products.append(product)
                    
                except Exception as e:
                    print(f"⚠️ Error parsing Walmart product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} real products on Walmart")
            return products
            
        except Exception as e:
            print(f"❌ Walmart search error: {e}")
            return []
    
    def search_ebay(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search eBay with fallback to mock data"""
        try:
            self.update_headers()
            search_url = f"https://www.ebay.com/sch/i.html?_nkw={quote_plus(query)}"
            time.sleep(random.uniform(1, 2))
            
            response = self.session.get(search_url, timeout=15)
            
            if response.status_code != 200:
                print(f"❌ eBay blocked request")
                return []
            
            soup = BeautifulSoup(response.content, 'html.parser')
            product_containers = soup.find_all('div', class_='s-item__wrapper')
            
            if not product_containers:
                print("❌ No eBay products found")
                return []
            
            products = []
            for container in product_containers[:max_results]:
                try:
                    name_elem = container.find('span', role='heading')
                    name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    price_elem = container.find('span', class_='s-item__price')
                    price = price_elem.get_text(strip=True) if price_elem else "Price not available"
                    
                    link_elem = container.find('a', class_='s-item__link')
                    url = link_elem['href'] if link_elem and link_elem.get('href') else ""
                    
                    img_elem = container.find('img')
                    image_url = img_elem.get('src') if img_elem else ""
                    
                    product = {
                        'site_name': 'ebay',
                        'product_name': name,
                        'price': price,
                        'rating': 'No rating',
                        'product_url': url,
                        'description': name,
                        'category': 'general',
                        'availability': 'Available',
                        'review_count': 'Unknown',
                        'image_url': image_url
                    }
                    
                    products.append(product)
                    
                except Exception as e:
                    print(f"⚠️ Error parsing eBay product: {e}")
                    continue
            
            print(f"✅ Found {len(products)} real products on eBay")
            return products
            
        except Exception as e:
            print(f"❌ eBay search error: {e}")
            return []
    
    def search_all_sites(self, query: str, max_results_per_site: int = 5) -> List[Dict]:
        """Search all supported e-commerce sites with robust fallbacks"""
        all_products = []
        
        print(f"🔍 Searching for '{query}' across multiple sites...")
        
        # Search Amazon (most likely to work)
        amazon_products = self.search_amazon(query, max_results_per_site)
        all_products.extend(amazon_products)
        
        # Search Walmart
        walmart_products = self.search_walmart(query, max_results_per_site)
        all_products.extend(walmart_products)
        
        # Search eBay
        ebay_products = self.search_ebay(query, max_results_per_site)
        all_products.extend(ebay_products)
        
        print(f"🎯 Total products found: {len(all_products)}")
        return all_products

# Maintain compatibility with existing API
class ProductScraper(RobustProductScraper):
    """Alias for backward compatibility"""
    pass
