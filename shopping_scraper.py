import requests
from bs4 import BeautifulSoup
import time
import random
from typing import List, Dict, Optional
import re
from urllib.parse import quote_plus, urljoin
import json

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
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Find product containers
            product_containers = soup.find_all('div', {'data-component-type': 's-search-result'})
            
            for container in product_containers[:max_results]:
                try:
                    # Product name
                    name_elem = container.find('h2', class_='a-size-mini')
                    if not name_elem:
                        name_elem = container.find('span', class_='a-size-medium')
                    product_name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = container.find('span', class_='a-price-whole')
                    if not price_elem:
                        price_elem = container.find('span', class_='a-offscreen')
                    price = price_elem.get_text(strip=True) if price_elem else "$0.00"
                    if not price.startswith('$'):
                        price = f"${price}"
                    
                    # Rating
                    rating_elem = container.find('span', class_='a-icon-alt')
                    rating = rating_elem.get_text(strip=True) if rating_elem else "No rating"
                    
                    # Review count
                    review_elem = container.find('span', class_='a-size-base')
                    review_count = review_elem.get_text(strip=True) if review_elem else "0 reviews"
                    
                    # Product URL
                    link_elem = container.find('h2').find('a') if container.find('h2') else None
                    product_url = urljoin('https://www.amazon.com', link_elem['href']) if link_elem else ""
                    
                    products.append({
                        'site_name': 'amazon',
                        'product_name': product_name,
                        'product_url': product_url,
                        'price': price,
                        'rating': rating,
                        'review_count': review_count,
                        'description': product_name,  # Use name as description for now
                        'category': self._extract_category(query),
                        'availability': 'In Stock'
                    })
                    
                except Exception as e:
                    print(f"Error parsing Amazon product: {e}")
                    continue
                    
        except Exception as e:
            print(f"Error searching Amazon: {e}")
            
        return products
    
    def search_walmart(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search Walmart for products"""
        products = []
        try:
            search_url = f"https://www.walmart.com/search?q={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Walmart uses different selectors, this is a simplified approach
            # In practice, you might need to use their API or more sophisticated scraping
            script_tags = soup.find_all('script', type='application/ld+json')
            
            for script in script_tags:
                try:
                    data = json.loads(script.string)
                    if isinstance(data, dict) and 'offers' in data:
                        product = {
                            'site_name': 'walmart',
                            'product_name': data.get('name', 'Unknown Product'),
                            'product_url': data.get('url', ''),
                            'price': f"${data.get('offers', {}).get('price', '0.00')}",
                            'rating': f"{data.get('aggregateRating', {}).get('ratingValue', '0')} stars",
                            'review_count': f"{data.get('aggregateRating', {}).get('reviewCount', '0')} reviews",
                            'description': data.get('description', '')[:200],
                            'category': self._extract_category(query),
                            'availability': 'Available'
                        }
                        products.append(product)
                        if len(products) >= max_results:
                            break
                except:
                    continue
                    
        except Exception as e:
            print(f"Error searching Walmart: {e}")
            
        return products
    
    def search_ebay(self, query: str, max_results: int = 10) -> List[Dict]:
        """Search eBay for products"""
        products = []
        try:
            search_url = f"https://www.ebay.com/sch/i.html?_nkw={quote_plus(query)}"
            response = self.session.get(search_url, timeout=10)
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Find product containers
            product_containers = soup.find_all('div', class_='s-item__wrapper')
            
            for container in product_containers[:max_results]:
                try:
                    # Product name
                    name_elem = container.find('h3', class_='s-item__title')
                    product_name = name_elem.get_text(strip=True) if name_elem else "Unknown Product"
                    
                    # Price
                    price_elem = container.find('span', class_='s-item__price')
                    price = price_elem.get_text(strip=True) if price_elem else "$0.00"
                    
                    # Product URL
                    link_elem = container.find('a', class_='s-item__link')
                    product_url = link_elem['href'] if link_elem else ""
                    
                    # Condition/description
                    condition_elem = container.find('span', class_='SECONDARY_INFO')
                    description = condition_elem.get_text(strip=True) if condition_elem else product_name
                    
                    products.append({
                        'site_name': 'ebay',
                        'product_name': product_name,
                        'product_url': product_url,
                        'price': price,
                        'rating': 'No rating',
                        'review_count': '0 reviews',
                        'description': description,
                        'category': self._extract_category(query),
                        'availability': 'Available'
                    })
                    
                except Exception as e:
                    print(f"Error parsing eBay product: {e}")
                    continue
                    
        except Exception as e:
            print(f"Error searching eBay: {e}")
            
        return products
    
    def _extract_category(self, query: str) -> str:
        """Extract category from search query"""
        query_lower = query.lower()
        
        categories = {
            'tent': 'camping',
            'camping': 'camping',
            'outdoor': 'camping',
            'laptop': 'electronics',
            'computer': 'electronics',
            'phone': 'electronics',
            'headphone': 'electronics',
            'book': 'books',
            'novel': 'books',
            'clothing': 'fashion',
            'shirt': 'fashion',
            'shoes': 'fashion',
            'kitchen': 'home',
            'furniture': 'home',
            'tool': 'tools',
            'car': 'automotive',
            'bike': 'sports',
            'fitness': 'sports'
        }
        
        for keyword, category in categories.items():
            if keyword in query_lower:
                return category
        
        return 'general'
    
    def search_all_sites(self, query: str, max_results_per_site: int = 5) -> List[Dict]:
        """Search all supported sites for products"""
        all_products = []
        
        print(f"🔍 Searching for: {query}")
        
        # Search Amazon
        print("📦 Searching Amazon...")
        amazon_products = self.search_amazon(query, max_results_per_site)
        all_products.extend(amazon_products)
        time.sleep(random.uniform(1, 2))  # Be respectful with requests
        
        # Search Walmart
        print("🏪 Searching Walmart...")
        walmart_products = self.search_walmart(query, max_results_per_site)
        all_products.extend(walmart_products)
        time.sleep(random.uniform(1, 2))
        
        # Search eBay
        print("🏷️ Searching eBay...")
        ebay_products = self.search_ebay(query, max_results_per_site)
        all_products.extend(ebay_products)
        
        print(f"✅ Found {len(all_products)} total products")
        return all_products

def extract_price_filter(query: str) -> tuple[Optional[float], Optional[float]]:
    """Extract price filters from natural language query"""
    query_lower = query.lower()
    
    # Look for "under X" patterns
    under_match = re.search(r'under\s+\$?(\d+(?:\.\d{2})?)', query_lower)
    if under_match:
        max_price = float(under_match.group(1))
        return None, max_price
    
    # Look for "between X and Y" patterns
    between_match = re.search(r'between\s+\$?(\d+(?:\.\d{2})?)\s+and\s+\$?(\d+(?:\.\d{2})?)', query_lower)
    if between_match:
        min_price = float(between_match.group(1))
        max_price = float(between_match.group(2))
        return min_price, max_price
    
    # Look for "over X" patterns
    over_match = re.search(r'over\s+\$?(\d+(?:\.\d{2})?)', query_lower)
    if over_match:
        min_price = float(over_match.group(1))
        return min_price, None
    
    return None, None

def filter_products_by_price(products: List[Dict], min_price: Optional[float], max_price: Optional[float]) -> List[Dict]:
    """Filter products by price range"""
    if not min_price and not max_price:
        return products
    
    filtered_products = []
    for product in products:
        try:
            # Extract numeric price
            price_str = product['price'].replace('$', '').replace(',', '')
            price = float(price_str)
            
            # Apply filters
            if min_price and price < min_price:
                continue
            if max_price and price > max_price:
                continue
                
            filtered_products.append(product)
        except:
            # If we can't parse price, include the product
            filtered_products.append(product)
    
    return filtered_products
