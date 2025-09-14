#!/usr/bin/env python3
"""
Demo script for the Shopping Tool
Shows how the natural language processing and web scraping works
"""

import sys
import os
sys.path.append('.')

from shopping_scraper import ProductScraper, extract_price_filter, filter_products_by_price
import json

def demo_query_parsing():
    """Demonstrate natural language query parsing"""
    print("🔍 Natural Language Query Parsing Demo")
    print("=" * 50)
    
    test_queries = [
        "find me the best tents under 100 dollars",
        "cheap laptops under 500",
        "top rated headphones between 50 and 200 dollars",
        "outdoor camping gear under 75",
        "best coffee makers over 100"
    ]
    
    for query in test_queries:
        print(f"\nQuery: '{query}'")
        
        # Extract price filters
        min_price, max_price = extract_price_filter(query)
        print(f"  Price range: ${min_price or 0} - ${max_price or '∞'}")
        
        # Extract search terms (simplified)
        import re
        clean_query = re.sub(r'\b(under|over|between|and|\$\d+(?:\.\d{2})?|dollars?|best|top|cheap)\b', '', query.lower())
        clean_query = re.sub(r'\s+', ' ', clean_query).strip()
        print(f"  Search terms: '{clean_query}'")
        
        # Determine sorting
        sort_by = 'relevance'
        if 'best' in query.lower() or 'top' in query.lower():
            sort_by = 'rating'
        elif 'cheap' in query.lower():
            sort_by = 'price_low'
        print(f"  Sort by: {sort_by}")

def demo_web_scraping():
    """Demonstrate web scraping (limited demo)"""
    print("\n\n🛒 Web Scraping Demo")
    print("=" * 50)
    
    scraper = ProductScraper()
    
    # Demo search (this will actually try to scrape)
    print("Searching for 'laptop' (limited results)...")
    try:
        products = scraper.search_amazon('laptop', max_results=3)
        
        if products:
            print(f"✅ Found {len(products)} products:")
            for i, product in enumerate(products, 1):
                print(f"  {i}. {product['product_name']}")
                print(f"     Price: {product['price']}")
                print(f"     Site: {product['site_name']}")
                print(f"     Rating: {product['rating']}")
        else:
            print("❌ No products found (this is normal for demo)")
    except Exception as e:
        print(f"⚠️ Scraping demo failed (expected): {e}")
        print("This is normal - real scraping requires proper headers and rate limiting")

def demo_price_filtering():
    """Demonstrate price filtering"""
    print("\n\n💰 Price Filtering Demo")
    print("=" * 50)
    
    # Sample products for filtering demo
    sample_products = [
        {'product_name': 'Budget Tent', 'price': '$45.99', 'site_name': 'amazon'},
        {'product_name': 'Premium Tent', 'price': '$129.99', 'site_name': 'amazon'},
        {'product_name': 'Basic Tent', 'price': '$75.50', 'site_name': 'walmart'},
        {'product_name': 'Luxury Tent', 'price': '$250.00', 'site_name': 'ebay'},
        {'product_name': 'Economy Tent', 'price': '$35.99', 'site_name': 'amazon'}
    ]
    
    print("Sample products:")
    for product in sample_products:
        print(f"  - {product['product_name']}: {product['price']}")
    
    # Test filtering under $100
    print(f"\nFiltering for products under $100:")
    filtered = filter_products_by_price(sample_products, None, 100.0)
    for product in filtered:
        print(f"  ✅ {product['product_name']}: {product['price']}")
    
    # Test filtering between $50-$150
    print(f"\nFiltering for products between $50-$150:")
    filtered = filter_products_by_price(sample_products, 50.0, 150.0)
    for product in filtered:
        print(f"  ✅ {product['product_name']}: {product['price']}")

def demo_api_structure():
    """Show what the API response would look like"""
    print("\n\n📡 API Response Structure Demo")
    print("=" * 50)
    
    # Sample API response
    sample_response = {
        "query": "find me the best tents under 100 dollars",
        "parsed_query": {
            "search_terms": "tents",
            "min_price": None,
            "max_price": 100.0,
            "sort_by": "rating"
        },
        "products": [
            {
                "site_name": "amazon",
                "product_name": "Coleman Sundome Tent",
                "product_url": "https://amazon.com/dp/B004J2GUOU",
                "price": "$89.99",
                "rating": "4.3 out of 5 stars",
                "review_count": "12,847 reviews",
                "description": "Easy setup dome tent for camping",
                "category": "camping",
                "availability": "In Stock"
            },
            {
                "site_name": "walmart",
                "product_name": "Ozark Trail 4-Person Tent",
                "product_url": "https://walmart.com/ip/tent123",
                "price": "$49.99",
                "rating": "4.1 stars",
                "review_count": "2,341 reviews",
                "description": "Affordable family camping tent",
                "category": "camping",
                "availability": "Available"
            }
        ],
        "total_found": 25,
        "filtered_count": 18,
        "returned_count": 2,
        "stored_in_db": 2,
        "timestamp": "2025-09-13T19:26:28-04:00"
    }
    
    print("Sample API Response:")
    print(json.dumps(sample_response, indent=2))

def main():
    """Run all demos"""
    print("🛒 Shopping Tool Demo")
    print("This demonstrates the core functionality without requiring AWS setup")
    print("\n")
    
    demo_query_parsing()
    demo_price_filtering()
    demo_api_structure()
    demo_web_scraping()  # This might fail, which is expected
    
    print("\n\n🎉 Demo Complete!")
    print("\nTo run the full shopping tool:")
    print("1. Set up your .env file with AWS credentials")
    print("2. Run: source shopping_env/bin/activate")
    print("3. Run: python shopping_api.py")
    print("4. Test with: curl -X POST http://localhost:5000/search -H 'Content-Type: application/json' -d '{\"query\": \"find me the best tents under 100 dollars\"}'")

if __name__ == "__main__":
    main()
