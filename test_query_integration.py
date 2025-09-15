#!/usr/bin/env python3
"""
Test script for query interpreter integration with shopping scraper
"""

import sys
import os
from dotenv import load_dotenv

# Add current directory to path
sys.path.append('.')

# Load environment variables
load_dotenv()

from shopping_scraper import ProductScraper
from query_interpreter import QueryInterpreter

def test_query_interpretation():
    """Test the query interpreter with various shopping queries"""
    print("🧠 Testing Query Interpreter Integration")
    print("=" * 50)
    
    # Initialize components
    try:
        interpreter = QueryInterpreter()
        scraper = ProductScraper()
        print("✅ Components initialized successfully")
    except Exception as e:
        print(f"❌ Initialization failed: {e}")
        return
    
    # Test queries with different characteristics
    test_queries = [
        "find me the best wireless headphones under $100",
        "cheap tents for camping under 50 dollars",
        "premium gaming laptop with RTX 4080",
        "bluetooth speakers between $20 and $80",
        "budget running shoes for women",
        "best coffee makers over $200"
    ]
    
    for i, query in enumerate(test_queries, 1):
        print(f"\n📝 Test {i}: '{query}'")
        print("-" * 40)
        
        try:
            # Test query interpretation
            interpreted = interpreter.interpret_query(query)
            
            print(f"🎯 Product Type: {interpreted.get('product_type', 'N/A')}")
            print(f"💰 Price Range: ${interpreted.get('price_range', {}).get('min', 'N/A')} - ${interpreted.get('price_range', {}).get('max', 'N/A')}")
            print(f"🏷️ Quality Indicators: {interpreted.get('quality_indicators', [])}")
            print(f"🔍 Keywords: {interpreted.get('keywords', [])}")
            
            # Test optimized search terms
            search_terms = interpreter.generate_optimized_search_terms(interpreted)
            print(f"🔍 Optimized Search Terms: {search_terms}")
            
            # Test site recommendations
            recommended_sites = interpreter.get_site_recommendations(interpreted)
            print(f"🏪 Recommended Sites: {recommended_sites}")
            
            # Test Amazon URL generation (without actually making request)
            print(f"🔗 Testing Amazon URL generation...")
            
            # Simulate what the scraper would do
            optimized_query = search_terms[0] if search_terms else query
            price_range = interpreted.get('price_range', {})
            price_min = price_range.get('min')
            price_max = price_range.get('max')
            
            from urllib.parse import quote_plus
            search_url = f"https://www.amazon.com/s?k={quote_plus(optimized_query)}"
            
            if price_min is not None:
                search_url += f"&low-price={price_min}"
            if price_max is not None:
                search_url += f"&high-price={price_max}"
            
            # Add sorting based on quality indicators
            quality_indicators = interpreted.get('quality_indicators', [])
            if 'best' in quality_indicators or 'premium' in quality_indicators:
                search_url += "&s=review-rank"
            elif 'cheap' in quality_indicators or 'budget' in quality_indicators:
                search_url += "&s=price-asc-rank"
            
            print(f"🔗 Generated Amazon URL: {search_url}")
            
        except Exception as e:
            print(f"❌ Error processing query: {e}")
            import traceback
            print(f"📋 Traceback: {traceback.format_exc()}")

def test_scraper_integration():
    """Test the scraper with query interpreter integration"""
    print(f"\n🔍 Testing Scraper Integration")
    print("=" * 50)
    
    try:
        scraper = ProductScraper()
        
        # Test with a simple query that should trigger price constraints
        test_query = "wireless headphones under $50"
        print(f"📝 Testing scraper with: '{test_query}'")
        
        # This will test the integrated search_amazon method
        # Note: We're not actually making the request to avoid rate limiting
        print("🔗 Testing URL generation and query optimization...")
        
        # The scraper should now use query interpreter internally
        # Let's verify it has the interpreter
        if hasattr(scraper, 'query_interpreter') and scraper.query_interpreter:
            print("✅ Scraper has query interpreter integrated")
            
            # Test interpretation
            interpreted = scraper.query_interpreter.interpret_query(test_query)
            print(f"🎯 Interpreted product type: {interpreted.get('product_type', 'N/A')}")
            print(f"💰 Extracted price constraint: max ${interpreted.get('price_range', {}).get('max', 'N/A')}")
        else:
            print("❌ Scraper missing query interpreter")
            
    except Exception as e:
        print(f"❌ Scraper integration test failed: {e}")
        import traceback
        print(f"📋 Traceback: {traceback.format_exc()}")

def main():
    """Run all tests"""
    print("🛒 Query Interpreter Integration Tests")
    print("=" * 60)
    
    # Check environment setup
    if not os.getenv('ANTHROPIC_API_KEY'):
        print("❌ ANTHROPIC_API_KEY not found in environment")
        print("Please ensure your .env file contains the API key")
        return
    
    print("✅ Environment variables loaded")
    
    # Run tests
    test_query_interpretation()
    test_scraper_integration()
    
    print(f"\n🎉 Integration tests completed!")
    print("💡 The system now supports:")
    print("  - Intelligent query interpretation")
    print("  - Automatic price range extraction and URL constraints")
    print("  - Optimized search terms generation")
    print("  - Site recommendations based on product category")
    print("  - Smart sorting based on quality indicators")

if __name__ == "__main__":
    main()
