#!/usr/bin/env python3
"""
Test script for the enhanced Amazon scraper with anti-detection measures.
This script demonstrates the stealth features including:
- User agent rotation
- Random delays
- Request retries with backoff
- Optional proxy support
"""

import sys
import os
sys.path.append(os.path.dirname(os.path.abspath(__file__)))

from shopping_scraper import ProductScraper
import json

def test_stealth_scraper():
    """Test the enhanced scraper with anti-detection measures"""
    print("🔍 Testing Enhanced Amazon Scraper with Anti-Detection")
    print("=" * 60)
    
    # Initialize scraper
    scraper = ProductScraper()
    
    # Optional: Add proxies if you have them
    # Example proxy list (these are dummy examples - replace with real proxies)
    # proxy_list = [
    #     "http://proxy1.example.com:8080",
    #     "http://proxy2.example.com:8080"
    # ]
    # scraper.add_proxies(proxy_list)
    
    # Test query
    test_query = "wireless headphones"
    
    print(f"🎯 Searching for: {test_query}")
    print("⏳ Using anti-detection measures...")
    print("   - Random user agent rotation")
    print("   - Request delays (1-2 seconds)")
    print("   - Retry logic with backoff")
    print("   - Realistic browser headers")
    
    # Search Amazon with stealth measures
    try:
        products = scraper.search_amazon(test_query, max_results=5)
        
        if products:
            print(f"\n✅ Successfully found {len(products)} products!")
            print("\n📦 Sample Results:")
            print("-" * 40)
            
            for i, product in enumerate(products[:3], 1):
                print(f"{i}. {product.get('product_name', 'Unknown')}")
                print(f"   Price: {product.get('price', 'N/A')}")
                print(f"   Rating: {product.get('rating', 'N/A')}")
                print()
            
            # Save results to file
            with open('stealth_scraper_results.json', 'w') as f:
                json.dump(products, f, indent=2)
            print("💾 Results saved to stealth_scraper_results.json")
            
        else:
            print("❌ No products found - this could indicate:")
            print("   - Amazon is blocking requests")
            print("   - Need to add proxy support")
            print("   - Query returned no results")
            
    except Exception as e:
        print(f"❌ Error during scraping: {e}")
        print("💡 Consider adding proxy support for better stealth")

def test_all_sites():
    """Test all sites with stealth measures"""
    print("\n🌐 Testing All Sites with Enhanced Stealth")
    print("=" * 60)
    
    scraper = ProductScraper()
    test_query = "bluetooth speaker"
    
    print(f"🎯 Searching for: {test_query}")
    
    try:
        all_products = scraper.search_all_sites(test_query, max_results=3)
        
        if all_products:
            print(f"\n✅ Found products from {len(set(p.get('site_name', 'unknown') for p in all_products))} sites")
            
            # Group by site
            by_site = {}
            for product in all_products:
                site = product.get('site_name', 'unknown')
                if site not in by_site:
                    by_site[site] = []
                by_site[site].append(product)
            
            for site, products in by_site.items():
                print(f"\n🏪 {site.title()} ({len(products)} products):")
                for product in products[:2]:
                    print(f"   • {product.get('product_name', 'Unknown')[:50]}...")
        else:
            print("❌ No products found from any site")
            
    except Exception as e:
        print(f"❌ Error during multi-site scraping: {e}")

if __name__ == "__main__":
    print("🚀 Enhanced Amazon Scraper Test Suite")
    print("🛡️  Anti-Detection Features Enabled")
    print()
    
    # Test Amazon specifically
    test_stealth_scraper()
    
    # Test all sites
    test_all_sites()
    
    print("\n" + "=" * 60)
    print("✨ Test completed!")
    print("💡 To add proxy support, uncomment and configure the proxy_list in the script")
