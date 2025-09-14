#!/usr/bin/env python3
"""
Complete Shopping Agent Workflow
Takes a natural language prompt, searches for products, and uploads to DynamoDB
"""

import sys
import os
import time
from datetime import datetime
from shopping_scraper import ProductScraper, extract_price_filter, filter_products_by_price
from shopping_api import clean_and_process_data, insert_products_to_dynamodb, DYNAMODB_AVAILABLE

sys.path.append("./data-pipeline/dynamodb")

from structureData import structure_data

def run_shopping_agent(prompt: str, max_results: int = 15):
    """
    Complete shopping workflow:
    1. Parse natural language prompt
    2. Search multiple e-commerce sites
    3. Filter and process results
    4. Upload to DynamoDB
    """
    
    print("🛒 Shopping Agent Starting...")
    print(f"📝 Query: '{prompt}'")
    print("=" * 60)
    
    # Step 1: Initialize scraper
    scraper = ProductScraper()
    
    # Step 2: Parse the query for price filters
    min_price, max_price = extract_price_filter(prompt)
    print(f"💰 Price range: ${min_price or 0} - ${max_price or '∞'}")
    
    # Step 3: Extract search terms (simplified)
    import re
    clean_query = re.sub(r'\b(under|over|between|and|\$\d+(?:\.\d{2})?|dollars?|best|top|cheap)\b', '', prompt.lower())
    clean_query = re.sub(r'\s+', ' ', clean_query).strip()
    search_terms = clean_query.replace('find me the', '').replace('find me', '').strip()
    
    print(f"🔍 Search terms: '{search_terms}'")
    
    # Step 4: Search all sites
    print("\n🌐 Searching e-commerce sites...")
    raw_products = scraper.search_all_sites(search_terms, max_results_per_site=max_results//3 + 1)
    
    if not raw_products:
        print("❌ No products found. This could be due to:")
        print("   - Anti-bot measures on websites")
        print("   - Network connectivity issues")
        print("   - Search terms too specific")
        print("\n💡 Try running with sample data instead:")
        print("   python run_shopping_agent.py --sample")
        return []
    
    print(f"✅ Found {len(raw_products)} raw products")
    
    # Step 5: Filter by price
    if min_price or max_price:
        print(f"\n💸 Filtering by price range...")
        filtered_products = filter_products_by_price(raw_products, min_price, max_price)
        print(f"✅ {len(filtered_products)} products match price criteria")
    else:
        filtered_products = raw_products
    
    # Step 6: Limit results
    final_products = filtered_products[:max_results]
    
    # Step 7: Show results
    print(f"\n📦 Top {len(final_products)} Products:")
    for i, product in enumerate(final_products, 1):
        print(f"  {i}. {product['product_name']}")
        print(f"     💲 {product['price']} | ⭐ {product['rating']} | 🏪 {product['site_name']}")
    
    # Step 8: Process and upload to DynamoDB
    if DYNAMODB_AVAILABLE:
        print(f"\n💾 Processing data for DynamoDB...")
        processed_products = clean_and_process_data(final_products)
        
        print(f"📤 Uploading {len(processed_products)} products to DynamoDB...")
        success_count = insert_products_to_dynamodb(processed_products)
        
        print(f"✅ Successfully uploaded {success_count} products to 'shopping_products_unstructured'")
    else:
        print("\n⚠️ DynamoDB not available - skipping upload")
        print("Make sure your .env file has valid AWS credentials")
    
    print(f"\n🎉 Shopping Agent Complete!")
    print(f"📊 Summary: Found {len(raw_products)} → Filtered {len(filtered_products)} → Uploaded {success_count if DYNAMODB_AVAILABLE else 0}")
    
    return final_products

def run_with_sample_data(prompt: str):
    """Run with sample data for testing"""
    
    print("🧪 Running with Sample Data (for testing)")
    print(f"📝 Query: '{prompt}'")
    print("=" * 60)
    
    # Sample products that match common queries
    sample_products = [
        {
            "site_name": "amazon",
            "product_name": "Coleman Sundome 4-Person Tent",
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
        },
        {
            "site_name": "amazon",
            "product_name": "CORE 6-Person Instant Cabin Tent",
            "product_url": "https://amazon.com/dp/B075RNBQX7",
            "price": "$95.00",
            "rating": "4.5 out of 5 stars",
            "review_count": "8,234 reviews",
            "description": "60-second setup instant tent",
            "category": "camping",
            "availability": "In Stock"
        },
        {
            "site_name": "ebay",
            "product_name": "REI Co-op Half Dome 2 Plus Tent",
            "product_url": "https://ebay.com/itm/tent456",
            "price": "$199.00",
            "rating": "4.7 stars",
            "review_count": "1,523 reviews",
            "description": "Lightweight backpacking tent",
            "category": "camping",
            "availability": "Available"
        }
    ]
    
    # Apply price filtering
    min_price, max_price = extract_price_filter(prompt)
    if min_price or max_price:
        print(f"💰 Filtering sample data by price: ${min_price or 0} - ${max_price or '∞'}")
        filtered_products = filter_products_by_price(sample_products, min_price, max_price)
    else:
        filtered_products = sample_products
    
    print(f"📦 Sample Products ({len(filtered_products)} found):")
    for i, product in enumerate(filtered_products, 1):
        print(f"  {i}. {product['product_name']}")
        print(f"     💲 {product['price']} | ⭐ {product['rating']} | 🏪 {product['site_name']}")
    
    # Upload to DynamoDB
    if DYNAMODB_AVAILABLE and filtered_products:
        print(f"\n💾 Processing sample data for DynamoDB...")
        processed_products = clean_and_process_data(filtered_products)
        
        print(f"📤 Uploading {len(processed_products)} products to DynamoDB...")
        success_count = insert_products_to_dynamodb(processed_products)
        
        print(f"✅ Successfully uploaded {success_count} products to 'shopping_products_unstructured'")
    else:
        print("\n⚠️ DynamoDB not available or no products to upload")
    
    return filtered_products

def main():
    """Main entry point"""
    
    if len(sys.argv) < 2:
        print("🛒 Shopping Agent - Natural Language Product Search")
        print("\nUsage:")
        print("  python run_shopping_agent.py \"find me the best tents under 100 dollars\"")
        print("  python run_shopping_agent.py --sample \"cheap laptops under 500\"")
        print("\nExamples:")
        print("  python run_shopping_agent.py \"find me the best tents under 100 dollars\"")
        print("  python run_shopping_agent.py \"cheap laptops under 500\"")
        print("  python run_shopping_agent.py \"top rated headphones between 50 and 200 dollars\"")
        print("  python run_shopping_agent.py --sample \"outdoor camping gear\"")
        return
    
    # Check for sample mode
    if sys.argv[1] == "--sample":
        if len(sys.argv) < 3:
            prompt = "find me the best tents under 100 dollars"
        else:
            prompt = sys.argv[2]
        run_with_sample_data(prompt)
    else:
        prompt = sys.argv[1]
        run_shopping_agent(prompt)
        structure_data()    # Replace the agent's response with structured data in the DB


if __name__ == "__main__":
    main()
