#!/usr/bin/env python3
"""
Working CUA Shopping Agent - Simple and Fast
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

# CUA imports
from computer import Computer
from agent import ComputerAgent

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

class WorkingShoppingAgent:
    def __init__(self):
        self.computer: Optional[Computer] = None
        self.agent: Optional[ComputerAgent] = None
        self.products_scraped = []
        self.start_time = 0.0
        
    async def initialize(self):
        """Initialize the CUA computer and agent"""
        print("🚀 Initializing Working Shopping Agent...")
        
        # Initialize computer interface
        self.computer = Computer(
            os_type="macos", 
            verbosity=logging.INFO,
            use_host_computer_server=True
        )
        
        await self.computer.__aenter__()
        
        # Create agent with Claude 4 Opus model (Claude 4)
        self.agent = ComputerAgent(
            model="claude-opus-4-20250514",
            tools=[self.computer],
            only_n_most_recent_images=2,
            verbosity=logging.INFO,
            trajectory_dir="shopping_trajectories",
            max_trajectory_budget=20.0
        )
        
        print("✅ Working agent initialized")
    
    async def navigate_and_search(self, query: str):
        """Navigate to Google Shopping and search for products - FAST MODE"""
        print("🧭 Agent navigating to search for:", query)
        
        navigation_prompt = f"""
URGENT: Search for "{query}" on Google Shopping as FAST as possible.

INSTRUCTIONS:
1. Take ONE screenshot
2. If search box visible: clear it, type "{query}", press Enter
3. If not on Google Shopping: go to shopping.google.com, then search
4. Wait 2 seconds for results
5. STOP - do not scroll or click anything else

BE FAST. NO EXTRA ACTIONS. NO SCROLLING. NO CLICKING PRODUCTS.
"""
        
        try:
            response_text = ""
            if self.agent is None:
                raise RuntimeError("Agent not initialized")
            async for chunk in self.agent.run(navigation_prompt):
                if isinstance(chunk, dict) and 'output' in chunk:
                    for output_item in chunk['output']:
                        if isinstance(output_item, dict):
                            # Check for content array with text
                            if 'content' in output_item and isinstance(output_item['content'], list):
                                for content_item in output_item['content']:
                                    if isinstance(content_item, dict) and 'text' in content_item:
                                        response_text += str(content_item['text'])
                            # Check for direct text content
                            elif 'text' in output_item:
                                response_text += str(output_item['text'])
                elif isinstance(chunk, str):
                    response_text += chunk
            
            print("✅ Navigation completed")
            return response_text
            
        except Exception as e:
            print(f"❌ Navigation error: {e}")
            return ""
    
    async def extract_products(self):
        """Let the agent extract products from current screen"""
        print("🤖 Agent extracting products...")
        
        extraction_prompt = """
EXTRACT ALL PRODUCTS VISIBLE NOW. Be FAST and COMPLETE.

Format each product as:
PRODUCT: [Name] - [Price] - [Store] - [Rating] - [Details]

Extract EVERY product you see. Don't take screenshots. Just extract from current view.
"""
        
        try:
            # Collect response from the streaming agent
            response_text = ""
            if self.agent is None:
                raise RuntimeError("Agent not initialized")
            async for chunk in self.agent.run(extraction_prompt):
                if isinstance(chunk, dict) and 'output' in chunk:
                    for output_item in chunk['output']:
                        if isinstance(output_item, dict):
                            # Check for content array with text
                            if 'content' in output_item and isinstance(output_item['content'], list):
                                for content_item in output_item['content']:
                                    if isinstance(content_item, dict) and 'text' in content_item:
                                        response_text += str(content_item['text'])
                            # Check for direct text content
                            elif 'text' in output_item:
                                response_text += str(output_item['text'])
                
            print(f"🔍 Final response_text length: {len(response_text)}")
            print(f"🔍 Sample extracted text: {response_text[:300]}...")
            
            # Count products found - look for actual product listings in the response
            product_count = max(
                response_text.count("PRODUCT:"),
                response_text.count('**PRODUCT:'),
                response_text.count('**PRODUCT 1:'),
                response_text.count('**PRODUCT 2:'),
                len([line for line in response_text.split('\n') if '**PRODUCT' in line or 'PRODUCT:' in line])
            )
            
            print(f"🔍 DEBUG: Response length: {len(response_text)} chars")
            print(f"🔍 DEBUG: Product count detected: {product_count}")
            print(f"🔍 DEBUG: Sample response: {response_text[:500]}...")
            
            # Save the extracted data
            product_blob = {
                "timestamp": int(time.time()),
                "raw_product_data": response_text,
                "product_count": product_count,
                "extraction_type": "working_agent_extraction",
                "runtime_seconds": int(time.time() - self.start_time)
            }
            
            self.products_scraped.append(product_blob)
            print(f"✅ Extracted {product_count} products")
            
            return product_count
            
        except Exception as e:
            print(f"❌ Extraction error: {e}")
            return 0
    
    async def scroll_and_extract_more(self):
        """Quick scroll and extract - NO DELAYS"""
        print("📜 Quick scroll for more products...")
        
        scroll_prompt = """
FAST SCROLL: Scroll down ONCE, extract ALL visible products immediately.

Format: PRODUCT: [Name] - [Price] - [Store] - [Rating] - [Details]

NO screenshots. NO waiting. Just scroll once and extract everything visible.
"""
        
        try:
            # Collect response from the streaming agent
            response_text = ""
            if self.agent is None:
                raise RuntimeError("Agent not initialized")
            async for chunk in self.agent.run(scroll_prompt):
                if isinstance(chunk, dict) and 'output' in chunk:
                    for output_item in chunk['output']:
                        if isinstance(output_item, dict):
                            # Check for content array with text
                            if 'content' in output_item and isinstance(output_item['content'], list):
                                for content_item in output_item['content']:
                                    if isinstance(content_item, dict) and 'text' in content_item:
                                        response_text += str(content_item['text'])
                            # Check for direct text content
                            elif 'text' in output_item:
                                response_text += str(output_item['text'])
            
            # Count products found - look for actual product listings in the response
            product_count = max(
                response_text.count("PRODUCT:"),
                len([line for line in response_text.split('\n') if '$' in line and any(word in line.lower() for word in ['tent', 'camping', 'outdoor'])]),
                response_text.count('**PRODUCT:')
            )
            
            # Save the extracted data
            product_blob = {
                "timestamp": int(time.time()),
                "raw_product_data": response_text,
                "product_count": product_count,
                "extraction_type": "working_agent_scroll_extraction",
                "runtime_seconds": int(time.time() - self.start_time)
            }
            
            self.products_scraped.append(product_blob)
            print(f"✅ Found {product_count} more products after scrolling")
            
            return product_count
            
        except Exception as e:
            print(f"❌ Scroll extraction error: {e}")
            return 0
    
    async def save_to_dynamodb(self):
        """Save all scraped data to DynamoDB"""
        print("📤 Saving to DynamoDB...")
        
        if not self.products_scraped:
            print("❌ No data to save!")
            return
        
        try:
            # Initialize DynamoDB
            dynamodb = boto3.resource('dynamodb', region_name='us-east-1')
            table = dynamodb.Table('shopping_products_unstructured')  # type: ignore
            
            saved_count = 0
            for i, product_blob in enumerate(self.products_scraped):
                try:
                    # Add unique ID and ensure all fields are properly formatted
                    item_to_save = {
                        'product_id': f"working_agent_{int(time.time())}_{i}",
                        'timestamp': product_blob.get('timestamp', int(time.time())),
                        'raw_product_data': str(product_blob.get('raw_product_data', '')),
                        'product_count': int(product_blob.get('product_count', 0)),
                        'extraction_type': str(product_blob.get('extraction_type', 'working_agent')),
                        'runtime_seconds': int(product_blob.get('runtime_seconds', 0))
                    }
                    
                    print(f"🔍 Saving item {i+1}: {len(item_to_save['raw_product_data'])} chars, {item_to_save['product_count']} products")
                    
                    # Upload to DynamoDB
                    table.put_item(Item=item_to_save)
                    saved_count += 1
                    print(f"✅ Successfully saved item {i+1}")
                    
                except Exception as e:
                    print(f"⚠️ Failed to save blob {i+1}: {e}")
                    print(f"   Blob data: {product_blob}")
            
            print(f"📤 Saved {saved_count}/{len(self.products_scraped)} data blobs")
            
        except Exception as e:
            print(f"❌ DynamoDB save error: {e}")
            import traceback
            traceback.print_exc()
    
    async def run_shopping_session(self, query: str):
        """Main method - run a complete shopping session"""
        self.start_time = time.time()
        
        try:
            await self.initialize()
            
            # Navigate and search
            await self.navigate_and_search(query)
            
            # Extract products from first screen
            total_products = await self.extract_products()
            
            # Quick 2 scrolls max for more data (60 second limit)
            scroll_count = 0
            max_scrolls = 2
            
            while scroll_count < max_scrolls and time.time() - self.start_time < 60:
                more_products = await self.scroll_and_extract_more()
                total_products += more_products
                scroll_count += 1
            
            runtime = time.time() - self.start_time
            print(f"\n⏰ Total runtime: {runtime:.1f}s")
            
            # Debug: Print what we're about to save
            print(f"\n🔍 DEBUG: About to save {len(self.products_scraped)} data blobs:")
            for i, blob in enumerate(self.products_scraped):
                print(f"   Blob {i+1}: {len(blob.get('raw_product_data', ''))} characters of data")
                print(f"   Sample: {blob.get('raw_product_data', '')[:200]}...")
            
            # Save everything to DynamoDB
            await self.save_to_dynamodb()
            
            print(f"\n🎉 Shopping session completed!")
            print(f"   Total products found: {total_products}")
            print(f"   Total runtime: {runtime:.1f}s")
            print(f"   Data blobs saved: {len(self.products_scraped)}")
            
            return self.products_scraped
            
        except Exception as e:
            print(f"❌ Shopping session failed: {e}")
            return []
        finally:
            if self.computer:
                await self.computer.__aexit__(None, None, None)

async def main():
    if len(sys.argv) < 2:
        print("Usage: python working_agent.py 'search query'")
        print("Example: python working_agent.py 'tents under 100'")
        sys.exit(1)
    
    query = sys.argv[1]
    agent = WorkingShoppingAgent()
    
    try:
        data_blobs = await agent.run_shopping_session(query)
        
        # Print summary
        total_products = sum(blob.get('product_count', 0) for blob in data_blobs)
        print(f"\n📊 FINAL SUMMARY:")
        print(f"   Total products extracted: {total_products}")
        print(f"   Data blobs saved: {len(data_blobs)}")
        
        for i, blob in enumerate(data_blobs, 1):
            product_count = blob.get('product_count', 0)
            extraction_type = blob.get('extraction_type', 'unknown')
            print(f"   Blob {i}: {product_count} products ({extraction_type})")
            
    except KeyboardInterrupt:
        print("\n⏹️ Session interrupted by user")
    except Exception as e:
        print(f"❌ Fatal error: {e}")

if __name__ == "__main__":
    asyncio.run(main())
