#!/usr/bin/env python3
"""
Debug scraper to test URL and image extraction
"""

from shopping_scraper import ProductScraper

def test_scraper():
    scraper = ProductScraper()
    
    print("🔍 Testing Amazon scraper...")
    products = scraper.search_amazon("wireless mouse", max_results=3)
    
    print(f"\n📊 Found {len(products)} products:")
    for i, product in enumerate(products):
        print(f"\n--- Product {i+1} ---")
        print(f"Name: {product['product_name']}")
        print(f"Price: {product['price']}")
        print(f"URL: {product['product_url']}")
        print(f"Image: {product['image_url']}")
        print(f"Rating: {product['rating']}")

if __name__ == "__main__":
    test_scraper()
