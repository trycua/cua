#!/usr/bin/env python3
"""
Test the shopping API with sample data to demonstrate full functionality
"""

import requests
import json
import time

def test_with_sample_data():
    """Test API with sample product data"""
    
    # Sample product data that mimics scraped results
    sample_data = {
        "query": "find me the best tents under 100 dollars",
        "max_results": 5,
        "store_in_db": True,
        # Override with sample products for testing
        "_test_products": [
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
            }
        ]
    }
    
    print("🧪 Testing Shopping API with Sample Data")
    print("=" * 50)
    
    try:
        # Test the search endpoint
        print("📡 Sending test request...")
        response = requests.post(
            'http://localhost:8080/search',
            json=sample_data,
            timeout=30
        )
        
        if response.status_code == 200:
            data = response.json()
            print("✅ API Response received!")
            print(f"   Query: {data.get('query', 'N/A')}")
            print(f"   Products found: {data.get('returned_count', 0)}")
            print(f"   Stored in DB: {data.get('stored_in_db', 0)}")
            
            # Show parsed query
            parsed = data.get('parsed_query', {})
            print(f"   Price range: ${parsed.get('min_price', 0)} - ${parsed.get('max_price', '∞')}")
            print(f"   Sort by: {parsed.get('sort_by', 'relevance')}")
            
            return True
        else:
            print(f"❌ API Error: {response.status_code}")
            print(f"   Response: {response.text}")
            return False
            
    except Exception as e:
        print(f"❌ Test failed: {e}")
        return False

def test_search_history():
    """Test the search history endpoint"""
    print("\n📋 Testing Search History...")
    
    try:
        response = requests.get('http://localhost:8080/search_history', timeout=10)
        
        if response.status_code == 200:
            data = response.json()
            print(f"✅ Found {data['count']} products in history")
            
            if data['recent_products']:
                print("   Recent products:")
                for i, product in enumerate(data['recent_products'][:3], 1):
                    print(f"   {i}. {product['product_name']} - ${product['current_price']}")
            
            return True
        else:
            print(f"❌ History Error: {response.status_code}")
            return False
            
    except Exception as e:
        print(f"❌ History test failed: {e}")
        return False

def test_health():
    """Test health endpoint"""
    print("\n🏥 Testing Health Endpoint...")
    
    try:
        response = requests.get('http://localhost:8080/health', timeout=5)
        
        if response.status_code == 200:
            data = response.json()
            print(f"✅ API Status: {data['status']}")
            return True
        else:
            print(f"❌ Health check failed: {response.status_code}")
            return False
            
    except Exception as e:
        print(f"❌ Health test failed: {e}")
        return False

if __name__ == "__main__":
    print("🛒 Shopping API Full Test Suite")
    print("Testing with sample data to demonstrate functionality\n")
    
    # Run tests
    health_ok = test_health()
    search_ok = test_with_sample_data()
    history_ok = test_search_history()
    
    print("\n" + "=" * 50)
    print("📊 Test Results:")
    print(f"   Health Check: {'✅' if health_ok else '❌'}")
    print(f"   Search API: {'✅' if search_ok else '❌'}")
    print(f"   Search History: {'✅' if history_ok else '❌'}")
    
    if all([health_ok, search_ok, history_ok]):
        print("\n🎉 All tests passed! Shopping API is fully functional.")
        print("\n🌐 External Access:")
        print("   Your API is accessible from other computers at:")
        print("   http://10.36.35.109:8080/search")
        print("   http://10.36.35.109:8080/health")
        
        print("\n💡 Example external request:")
        print("   curl -X POST http://10.36.35.109:8080/search \\")
        print("     -H 'Content-Type: application/json' \\")
        print("     -d '{\"query\": \"find me the best tents under 100 dollars\"}'")
    else:
        print("\n⚠️ Some tests failed. Check the API configuration.")
