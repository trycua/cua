#!/usr/bin/env python3
"""
Hybrid Shopping Agent - AI Query Interpretation + Fast Traditional Scraping
"""

import asyncio
import json
import logging
import os
import sys
import time
from typing import List, Dict, Any, Optional

# Load environment variables
from dotenv import load_dotenv
load_dotenv('.env.local')

# CUA imports for query interpretation
from computer import Computer
from agent import ComputerAgent

# Import our existing fast scraper
from shopping_scraper import ProductScraper

# DynamoDB imports
import boto3

# Set AWS credentials from environment
aws_key = os.getenv('AMAZON_API_KEY')
aws_secret = os.getenv('AMAZON_SECRET_KEY')
aws_region = os.getenv('AWS_REGION', 'us-east-1')

if aws_key:
    os.environ['AWS_ACCESS_KEY_ID'] = aws_key
if aws_secret:
    os.environ['AWS_SECRET_ACCESS_KEY'] = aws_secret
if aws_region:
    os.environ['AWS_DEFAULT_REGION'] = aws_region


class QueryInterpreter:
    """Uses CUA agent to interpret natural language shopping queries"""
    
    def __init__(self):
        self.computer: Optional[Computer] = None
        self.agent: Optional[ComputerAgent] = None
    
    async def initialize(self):
        """Initialize the CUA computer and agent for query interpretation"""
        print("🧠 Initializing Query Interpreter...")
        
        # Initialize computer interface
        self.computer = Computer(
            os_type="macos", 
            verbosity=logging.WARNING,  # Reduce verbosity for faster operation
            use_host_computer_server=True
        )
        
        await self.computer.__aenter__()
        
        # Create agent with Claude 4 model
        self.agent = ComputerAgent(
            model="claude-sonnet-4-20250514",
            tools=[self.computer],
            only_n_most_recent_images=1,
            verbosity=logging.WARNING,
            max_trajectory_budget=5.0  # Small budget for quick interpretation
        )
        
        print("✅ Query Interpreter initialized")
    
    async def interpret_query(self, user_query: str) -> Dict[str, Any]:
        """Interpret natural language query and extract structured search parameters"""
        print(f"🔍 Interpreting query: '{user_query}'")
        
        interpretation_prompt = f"""
TASK: Analyze this shopping query and extract structured search parameters.

USER QUERY: "{user_query}"

Extract and format as JSON:
{{
    "search_terms": ["main keyword", "secondary keywords"],
    "price_range": {{"min": null, "max": 100}},  // null if not specified
    "category": "general",  // or specific category like "electronics", "clothing", etc.
    "sites": ["amazon", "walmart", "ebay"],  // which sites to search
    "filters": {{
        "brand": null,
        "rating_min": null,
        "features": []
    }},
    "intent": "find products",  // brief description of user intent
    "urgency": "normal"  // "high" if user wants fast results
}}

RULES:
- Extract price ranges from phrases like "under $100", "between $50-200", "cheap", "budget"
- Identify specific brands, features, or requirements
- Default to searching all sites unless user specifies
- Be concise and accurate
- If unclear, make reasonable assumptions

OUTPUT ONLY THE JSON, NO OTHER TEXT.
"""
        
        try:
            response_text = ""
            if self.agent is None:
                raise RuntimeError("Agent not initialized")
            
            async for chunk in self.agent.run(interpretation_prompt):
                if isinstance(chunk, dict) and 'output' in chunk:
                    for output_item in chunk['output']:
                        if isinstance(output_item, dict):
                            if 'content' in output_item and isinstance(output_item['content'], list):
                                for content_item in output_item['content']:
                                    if isinstance(content_item, dict) and 'text' in content_item:
                                        response_text += str(content_item['text'])
                            elif 'text' in output_item:
                                response_text += str(output_item['text'])
                elif isinstance(chunk, str):
                    response_text += chunk
            
            # Extract JSON from response
            try:
                # Find JSON in the response
                json_start = response_text.find('{')
                json_end = response_text.rfind('}') + 1
                if json_start >= 0 and json_end > json_start:
                    json_str = response_text[json_start:json_end]
                    parsed_query = json.loads(json_str)
                    print(f"✅ Query interpreted: {parsed_query}")
                    return parsed_query
                else:
                    raise ValueError("No JSON found in response")
            except (json.JSONDecodeError, ValueError) as e:
                print(f"⚠️ Failed to parse JSON response: {e}")
                print(f"Raw response: {response_text}")
                # Fallback to basic interpretation
                return self._fallback_interpretation(user_query)
            
        except Exception as e:
            print(f"❌ Query interpretation error: {e}")
            return self._fallback_interpretation(user_query)
    
    def _fallback_interpretation(self, user_query: str) -> Dict[str, Any]:
        """Fallback interpretation using simple keyword matching"""
        query_lower = user_query.lower()
        
        # Extract price range - improved logic
        price_range: Dict[str, Any] = {"min": None, "max": None}
        import re
        under_match = re.search(r'under\s+\$?(\d+)', query_lower)
        if under_match:
            price_range["max"] = int(under_match.group(1))
        
        # Basic search terms
        search_terms = [word for word in user_query.split() if len(word) > 2 and word.lower() not in ["the", "and", "for", "under", "over", "with"]]
        
        return {
            "search_terms": search_terms[:3],  # Take first 3 meaningful words
            "price_range": price_range,
            "category": "general",
            "sites": ["amazon", "walmart", "ebay"],
            "filters": {"brand": None, "rating_min": None, "features": []},
            "intent": "find products",
            "urgency": "normal"
        }
    
    async def cleanup(self):
        """Clean up resources"""
        if self.computer:
            await self.computer.__aexit__(None, None, None)


class HybridShoppingAgent:
    """Main hybrid agent that combines AI interpretation with fast scraping"""
    
    def __init__(self):
        self.interpreter = QueryInterpreter()
        self.scraper = ProductScraper()
        self.start_time = 0.0
    
    async def search_products(self, user_query: str, max_products: int = 20) -> List[Dict[str, Any]]:
        """Main method: interpret query and search for products"""
        self.start_time = time.time()
        
        try:
            # Step 1: Initialize query interpreter
            await self.interpreter.initialize()
            
            # Step 2: Interpret the natural language query
            interpreted_query = await self.interpreter.interpret_query(user_query)
            print(f"🎯 Interpreted query: {interpreted_query}")
            
            # Step 3: Convert to search terms for traditional scraper
            search_terms = " ".join(interpreted_query.get("search_terms", [user_query]))
            print(f"🔍 Searching for: '{search_terms}'")
            
            # Step 4: Use traditional scraper for fast results
            all_products = []
            sites_to_search = interpreted_query.get("sites", ["amazon"])
            
            for site in sites_to_search:
                if len(all_products) >= max_products:
                    break
                
                print(f"🛒 Searching {site}...")
                try:
                    if site == "amazon":
                        products = self.scraper.search_amazon(search_terms, max_products//len(sites_to_search))
                    elif site == "walmart":
                        products = self.scraper.search_walmart(search_terms, max_products//len(sites_to_search))
                    elif site == "ebay":
                        products = self.scraper.search_ebay(search_terms, max_products//len(sites_to_search))
                    else:
                        continue
                    
                    # Filter by price range if specified
                    price_range = interpreted_query.get("price_range", {})
                    if price_range.get("max"):
                        products = self._filter_by_price(products, price_range)
                    
                    all_products.extend(products)
                    print(f"✅ Found {len(products)} products on {site}")
                    
                except Exception as e:
                    print(f"⚠️ Error scraping {site}: {e}")
                    continue
            
            # Step 5: Save to DynamoDB
            await self._save_results(user_query, interpreted_query, all_products)
            
            runtime = time.time() - self.start_time
            print(f"\n🎉 Hybrid search completed!")
            print(f"   Products found: {len(all_products)}")
            print(f"   Runtime: {runtime:.1f}s")
            
            return all_products[:max_products]
            
        except Exception as e:
            print(f"❌ Hybrid search failed: {e}")
            return []
        finally:
            await self.interpreter.cleanup()
    
    def _filter_by_price(self, products: List[Dict[str, Any]], price_range: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Filter products by price range"""
        filtered = []
        max_price = price_range.get("max")
        min_price = price_range.get("min")
        
        for product in products:
            try:
                price_str = product.get("price", "").replace("$", "").replace(",", "")
                if price_str and price_str.replace(".", "").isdigit():
                    price = float(price_str)
                    
                    if max_price and price > max_price:
                        continue
                    if min_price and price < min_price:
                        continue
                    
                    filtered.append(product)
                else:
                    # Include products without clear pricing
                    filtered.append(product)
            except:
                # Include products with parsing errors
                filtered.append(product)
        
        return filtered
    
    async def _save_results(self, original_query: str, interpreted_query: Dict[str, Any], products: List[Dict[str, Any]]):
        """Save search results to DynamoDB"""
        try:
            dynamodb = boto3.resource('dynamodb', region_name='us-east-1')
            table = dynamodb.Table('shopping_products_unstructured')  # type: ignore
            
            # Save the hybrid search result
            result_item = {
                'product_id': f"hybrid_search_{int(time.time())}",
                'timestamp': int(time.time()),
                'original_query': original_query,
                'interpreted_query': json.dumps(interpreted_query),
                'products_found': len(products),
                'products_data': json.dumps(products),
                'extraction_type': 'hybrid_agent_search',
                'runtime_seconds': int(time.time() - self.start_time)
            }
            
            table.put_item(Item=result_item)
            print(f"📤 Saved search results to DynamoDB")
            
        except Exception as e:
            print(f"⚠️ Failed to save to DynamoDB: {e}")


async def main():
    if len(sys.argv) < 2:
        print("Usage: python hybrid_shopping_agent.py 'search query'")
        print("Examples:")
        print("  python hybrid_shopping_agent.py 'camping tents under 100 dollars'")
        print("  python hybrid_shopping_agent.py 'wireless headphones with noise cancellation'")
        print("  python hybrid_shopping_agent.py 'cozy blankets for winter'")
        sys.exit(1)
    
    user_query = sys.argv[1]
    agent = HybridShoppingAgent()
    
    try:
        products = await agent.search_products(user_query, max_products=20)
        
        # Print results summary
        print(f"\n📊 SEARCH RESULTS:")
        print(f"   Query: '{user_query}'")
        print(f"   Products found: {len(products)}")
        
        # Show top 5 products
        for i, product in enumerate(products[:5], 1):
            name = product.get('product_name', 'Unknown')[:50]
            price = product.get('price', 'N/A')
            site = product.get('site_name', 'Unknown')
            print(f"   {i}. {name}... - {price} ({site})")
        
        if len(products) > 5:
            print(f"   ... and {len(products) - 5} more products")
            
    except KeyboardInterrupt:
        print("\n⏹️ Search interrupted by user")
    except Exception as e:
        print(f"❌ Fatal error: {e}")


if __name__ == "__main__":
    asyncio.run(main())
