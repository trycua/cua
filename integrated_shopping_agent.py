#!/usr/bin/env python3
"""
Integrated Shopping Agent - Combines CUA working_agent with AI query processing
"""

import asyncio
import sys
import time
from typing import Dict, List, Optional
from ai_query_processor import AIQueryProcessor
from working_agent import WorkingShoppingAgent

class IntegratedShoppingAgent:
    def __init__(self):
        self.query_processor = AIQueryProcessor()
        self.cua_agent = WorkingShoppingAgent()
        
    async def run_smart_shopping_session(self, natural_query: str) -> Dict:
        """
        Complete shopping workflow:
        1. Parse natural language query with AI
        2. Use CUA agent to search and extract products
        3. Return structured results
        """
        print(f"🛒 Integrated Shopping Agent Starting...")
        print(f"📝 Query: '{natural_query}'")
        print("=" * 60)
        
        start_time = time.time()
        
        try:
            # Step 1: Parse query with AI (optional, fallback to basic parsing)
            print("🧠 Processing query with AI...")
            try:
                parsed_query = self.query_processor.enhance_query_parsing(natural_query)
                print(f"✅ AI parsed query: {parsed_query}")
            except Exception as e:
                print(f"⚠️ AI parsing failed, using fallback: {e}")
                parsed_query = self.query_processor._fallback_parsing(natural_query)
            
            # Step 2: Extract clean search terms for CUA agent
            search_terms = parsed_query.get('search_terms', natural_query)
            product_type = parsed_query.get('product_type', 'products')
            price_constraints = {
                'min_price': parsed_query.get('min_price'),
                'max_price': parsed_query.get('max_price')
            }
            
            print(f"🔍 Search terms for CUA: '{search_terms}'")
            print(f"💰 Price constraints: {price_constraints}")
            
            # Step 3: Run CUA agent to search and extract
            print("\n🤖 Starting CUA agent search...")
            product_blobs = await self.cua_agent.run_shopping_session(search_terms)
            
            # Step 4: Process results
            total_runtime = time.time() - start_time
            total_products = sum(blob.get('product_count', 0) for blob in product_blobs)
            
            results = {
                'success': len(product_blobs) > 0,
                'query': natural_query,
                'parsed_query': parsed_query,
                'search_terms_used': search_terms,
                'total_products_found': total_products,
                'data_blobs_saved': len(product_blobs),
                'runtime_seconds': round(total_runtime, 2),
                'raw_data_blobs': product_blobs,
                'summary': f"Found {total_products} products in {total_runtime:.1f}s"
            }
            
            # Step 5: Generate suggestions for improvement
            if total_products == 0:
                suggestions = self.query_processor.generate_search_suggestions(natural_query, [])
                results['suggestions'] = suggestions
                print(f"\n💡 Suggestions: {suggestions}")
            
            print(f"\n🎉 Integrated Shopping Session Complete!")
            print(f"   Total products found: {total_products}")
            print(f"   Total runtime: {total_runtime:.1f}s")
            print(f"   Data blobs saved to DynamoDB: {len(product_blobs)}")
            
            return results
            
        except Exception as e:
            print(f"❌ Integrated shopping session failed: {e}")
            import traceback
            traceback.print_exc()
            
            return {
                'success': False,
                'error': str(e),
                'query': natural_query,
                'runtime_seconds': time.time() - start_time
            }

async def main():
    """Main entry point for integrated shopping agent"""
    if len(sys.argv) < 2:
        print("🛒 Integrated Shopping Agent - AI + CUA")
        print("\nUsage:")
        print("  python integrated_shopping_agent.py \"find me the best tents under 100 dollars\"")
        print("\nExamples:")
        print("  python integrated_shopping_agent.py \"find me the best tents under 100 dollars\"")
        print("  python integrated_shopping_agent.py \"cheap laptops under 500\"")
        print("  python integrated_shopping_agent.py \"top rated headphones between 50 and 200 dollars\"")
        return
    
    query = sys.argv[1]
    agent = IntegratedShoppingAgent()
    
    try:
        results = await agent.run_smart_shopping_session(query)
        
        # Print final summary
        print(f"\n📊 FINAL RESULTS:")
        print(f"   Success: {results.get('success', False)}")
        print(f"   Query: {results.get('query', 'Unknown')}")
        print(f"   Products found: {results.get('total_products_found', 0)}")
        print(f"   Runtime: {results.get('runtime_seconds', 0)}s")
        
        if not results.get('success'):
            print(f"   Error: {results.get('error', 'Unknown error')}")
            if 'suggestions' in results:
                print(f"   Suggestions: {results['suggestions']}")
                
    except KeyboardInterrupt:
        print("\n⏹️ Session interrupted by user")
    except Exception as e:
        print(f"❌ Fatal error: {e}")

if __name__ == "__main__":
    asyncio.run(main())
