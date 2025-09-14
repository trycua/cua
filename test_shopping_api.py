#!/usr/bin/env python3
"""
Test script for the Shopping API
Run this to test the complete workflow
"""

import requests
import json
import time
from dotenv import load_dotenv
import os

# Load environment variables
load_dotenv()

def test_api_health():
    """Test if the API is running"""
    try:
        response = requests.get('http://localhost:5000/health', timeout=5)
        if response.status_code == 200:
            print("✅ API is healthy")
            return True
        else:
            print(f"❌ API health check failed: {response.status_code}")
            return False
    except requests.exceptions.RequestException as e:
        print(f"❌ Cannot connect to API: {e}")
        return False

def test_search_query(query: str):
    """Test a search query"""
    print(f"\n🔍 Testing query: '{query}'")
    
    try:
        response = requests.post(
            'http://localhost:5000/search',
            json={
                'query': query,
                'max_results': 10,
                'store_in_db': True
            },
            timeout=30
        )
        
        if response.status_code == 200:
            data = response.json()
            print(f"✅ Search successful!")
            print(f"   Found: {data['total_found']} total products")
            print(f"   Filtered: {data['filtered_count']} products")
            print(f"   Returned: {data['returned_count']} products")
            print(f"   Stored in DB: {data['stored_in_db']} products")
            
            # Show first few products
            if data['products']:
                print("\n📦 Sample products:")
                for i, product in enumerate(data['products'][:3]):
                    print(f"   {i+1}. {product['product_name']} - {product['price']} ({product['site_name']})")
            
            return True
        else:
            print(f"❌ Search failed: {response.status_code}")
            print(f"   Error: {response.text}")
            return False
            
    except requests.exceptions.RequestException as e:
        print(f"❌ Request failed: {e}")
        return False

def test_search_history():
    """Test search history endpoint"""
    print(f"\n📋 Testing search history...")
    
    try:
        response = requests.get('http://localhost:5000/search_history', timeout=10)
        
        if response.status_code == 200:
            data = response.json()
            print(f"✅ Search history retrieved!")
            print(f"   Found: {data['count']} recent products")
            
            if data['recent_products']:
                print("\n🕒 Recent products:")
                for i, product in enumerate(data['recent_products'][:3]):
                    print(f"   {i+1}. {product['product_name']} - ${product['current_price']} ({product['site_name']})")
            
            return True
        else:
            print(f"❌ Search history failed: {response.status_code}")
            return False
            
    except requests.exceptions.RequestException as e:
        print(f"❌ Request failed: {e}")
        return False

def test_category_search():
    """Test category search endpoint"""
    print(f"\n🏷️ Testing category search...")
    
    try:
        response = requests.get(
            'http://localhost:5000/search_by_category?category=camping&price_range=50-100',
            timeout=10
        )
        
        if response.status_code == 200:
            data = response.json()
            print(f"✅ Category search successful!")
            print(f"   Found: {data['count']} products in camping category")
            return True
        else:
            print(f"❌ Category search failed: {response.status_code}")
            return False
            
    except requests.exceptions.RequestException as e:
        print(f"❌ Request failed: {e}")
        return False

def check_environment():
    """Check if required environment variables are set"""
    print("🔧 Checking environment variables...")
    
    required_vars = ['AMAZON_API_KEY', 'AMAZON_SECRET_KEY', 'AWS_REGION']
    optional_vars = ['OPENAI_API_KEY', 'ANTHROPIC_API_KEY']
    
    missing_required = []
    missing_optional = []
    
    for var in required_vars:
        if not os.getenv(var):
            missing_required.append(var)
        else:
            print(f"   ✅ {var} is set")
    
    for var in optional_vars:
        if not os.getenv(var):
            missing_optional.append(var)
        else:
            print(f"   ✅ {var} is set")
    
    if missing_required:
        print(f"   ❌ Missing required variables: {', '.join(missing_required)}")
        print("   Please set these in your .env file")
        return False
    
    if missing_optional:
        print(f"   ⚠️ Missing optional variables: {', '.join(missing_optional)}")
        print("   These are optional but recommended for full functionality")
    
    return True

def main():
    """Run all tests"""
    print("🚀 Testing Shopping API")
    print("=" * 50)
    
    # Check environment
    if not check_environment():
        print("\n❌ Environment check failed. Please fix your .env file first.")
        return
    
    # Check if API is running
    if not test_api_health():
        print("\n❌ API is not running. Please start it first:")
        print("   python shopping_api.py")
        return
    
    # Test search queries
    test_queries = [
        "find me the best tents under 100 dollars",
        "cheap laptops under 500",
        "top rated headphones between 50 and 200 dollars"
    ]
    
    for query in test_queries:
        test_search_query(query)
        time.sleep(2)  # Be nice to the API
    
    # Test other endpoints
    test_search_history()
    test_category_search()
    
    print("\n🎉 Testing completed!")
    print("\n💡 Try these curl commands:")
    print('curl -X POST http://localhost:5000/search -H "Content-Type: application/json" -d \'{"query": "find me the best tents under 100 dollars"}\'')
    print('curl http://localhost:5000/search_history')

if __name__ == "__main__":
    main()
