#!/usr/bin/env python3
"""
Shopping API - Core functionality for product search and data processing
Supports both traditional scraping and CUA (Computer-Use Agent) modes
"""

import os
import time
import asyncio
import subprocess
import json
import signal
from concurrent.futures import ThreadPoolExecutor, TimeoutError
from threading import Timer
import boto3
from decimal import Decimal
from typing import List, Dict, Any, Optional
from dotenv import load_dotenv

load_dotenv('.env.local')

# Check DynamoDB availability
try:
    # Set AWS credentials from environment
    aws_key = os.getenv('AMAZON_API_KEY')
    aws_secret = os.getenv('AMAZON_SECRET_KEY')
    aws_region = os.getenv('AWS_REGION', 'ca-central-1')

    if aws_key:
        os.environ['AWS_ACCESS_KEY_ID'] = aws_key
    if aws_secret:
        os.environ['AWS_SECRET_ACCESS_KEY'] = aws_secret
    if aws_region:
        os.environ['AWS_DEFAULT_REGION'] = aws_region

    # Test DynamoDB connection
    dynamodb = boto3.resource('dynamodb', region_name=aws_region)
    DYNAMODB_AVAILABLE = True
    print("✅ DynamoDB connection available")
except Exception as e:
    DYNAMODB_AVAILABLE = False
    print(f"⚠️ DynamoDB not available: {e}")

def clean_and_process_data(products: List[Dict]) -> List[Dict]:
    """Clean and standardize product data for storage"""
    processed_products = []
    
    for product in products:
        try:
            # Extract price as Decimal (DynamoDB compatible)
            price_str = product.get('price', '0')
            price_clean = price_str.replace('$', '').replace(',', '')
            try:
                price_decimal = Decimal(str(price_clean))
            except:
                price_decimal = Decimal('0.0')
            
            # Clean rating as Decimal (DynamoDB compatible)
            rating_str = product.get('rating', '0')
            if 'out of' in rating_str:
                rating_clean = rating_str.split(' ')[0]
            else:
                rating_clean = rating_str.replace(' stars', '')
            
            try:
                rating_decimal = Decimal(str(rating_clean))
            except:
                rating_decimal = Decimal('0.0')
            
            processed_product = {
                'product_id': f"{product.get('site_name', 'unknown')}_{int(time.time())}_{len(processed_products)}",
                'product_name': str(product.get('product_name', 'Unknown Product')),
                'price': price_decimal,
                'price_str': price_str,
                'rating': rating_decimal,
                'rating_str': rating_str,
                'site_name': str(product.get('site_name', 'unknown')),
                'product_url': str(product.get('product_url', '')),
                'image_url': str(product.get('image_url', '')),
                'description': str(product.get('description', '')),
                'category': str(product.get('category', 'general')),
                'availability': str(product.get('availability', 'unknown')),
                'review_count': str(product.get('review_count', '0')),
                'timestamp': Decimal(str(int(time.time()))),
                'extraction_type': 'shopping_api_processed'
            }
            
            processed_products.append(processed_product)
            
        except Exception as e:
            print(f"⚠️ Error processing product: {e}")
            continue
    
    return processed_products

def insert_products_to_dynamodb(products: List[Dict]) -> int:
    """Insert processed products into DynamoDB"""
    if not DYNAMODB_AVAILABLE:
        print("❌ DynamoDB not available")
        return 0
    
    try:
        dynamodb = boto3.resource('dynamodb', region_name=os.getenv('AWS_REGION', 'ca-central-1'))
        table = dynamodb.Table('shopping_products_unstructured')
        
        success_count = 0
        for product in products:
            try:
                table.put_item(Item=product)
                success_count += 1
            except Exception as e:
                print(f"⚠️ Failed to insert product {product.get('product_id', 'unknown')}: {e}")
        
        return success_count
        
    except Exception as e:
        print(f"❌ DynamoDB insertion error: {e}")
        return 0

def run_traditional_scraper(query: str, max_results: int = 20) -> List[Dict]:
    """Run traditional web scraper mode with query interpretation"""
    print(f"🔍 Running traditional scraper for: '{query}'")
    
    try:
        from shopping_scraper import ProductScraper
        from query_interpreter import QueryInterpreter
        
        # Use query interpreter to optimize the search
        query_interpreter = QueryInterpreter()
        interpreted_query = query_interpreter.interpret_query(query)
        print(f"🧠 Query interpretation: {interpreted_query.get('product_type', 'N/A')}")
        
        # Generate optimized search terms
        search_terms = query_interpreter.generate_optimized_search_terms(interpreted_query)
        print(f"🔍 Optimized search terms: {search_terms}")
        
        # Use the first optimized search term (or original query if none)
        search_query = search_terms[0] if search_terms else query
        print(f"🎯 Using search term: '{search_query}'")
        
        scraper = ProductScraper()
        products = scraper.search_all_sites(search_query, max_results_per_site=max_results//3)
        
        # Process and clean the data
        processed_products = clean_and_process_data(products)
        
        print(f"✅ Traditional scraper found {len(processed_products)} products")
        return processed_products
        
    except Exception as e:
        print(f"❌ Traditional scraper error: {e}")
        return []

def run_cua_scraper_with_timeout(query: str, timeout_seconds: int = 45, fast_mode: bool = True) -> List[Dict]:
    """Run CUA scraper with timeout using subprocess - optimized for speed"""
    print(f"🤖 Running FAST CUA scraper for: '{query}' (timeout: {timeout_seconds}s)")
    
    try:
        # Run working_agent.py as subprocess with timeout and fast mode
        cmd = ['python', 'working_agent.py', query]
        if fast_mode:
            cmd.append('--fast')  # Add fast mode flag
        
        process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            cwd=os.getcwd()
        )
        
        try:
            stdout, stderr = process.communicate(timeout=timeout_seconds)
            
            if process.returncode == 0:
                print(f"✅ FAST CUA scraper completed successfully")
                print(f"📝 CUA output: {stdout[-300:]}...")  # Show last 300 chars
                
                # CUA scraper saves directly to DynamoDB, so we return a summary
                return [{
                    'product_id': f'cua_scraper_{int(time.time())}',
                    'product_name': f'CUA Scraper Results for: {query}',
                    'price': Decimal('0.0'),
                    'price_str': 'See DynamoDB',
                    'rating': Decimal('0.0'),
                    'rating_str': 'N/A',
                    'site_name': 'cua_scraper',
                    'product_url': '',
                    'image_url': '',
                    'description': f'CUA scraper executed for query: {query}',
                    'category': 'cua_results',
                    'availability': 'completed',
                    'review_count': '0',
                    'timestamp': Decimal(str(int(time.time()))),
                    'extraction_type': 'cua_scraper_summary',
                    'cua_stdout': stdout,
                    'cua_stderr': stderr,
                    'fast_mode': fast_mode
                }]
            else:
                print(f"❌ CUA scraper failed with return code {process.returncode}")
                print(f"📝 Error output: {stderr}")
                return []
                
        except subprocess.TimeoutExpired:
            print(f"⏰ CUA scraper timed out after {timeout_seconds} seconds")
            process.kill()
            process.communicate()  # Clean up
            return []
            
    except Exception as e:
        print(f"❌ CUA scraper execution error: {e}")
        return []

def dual_mode_search(query: str, mode: str = "auto", max_results: int = 20, cua_timeout: int = 45) -> Dict[str, Any]:
    """
    Dual-mode search with CUA fallback when traditional scrapers fail
    
    Args:
        query: Search query
        mode: "traditional", "cua", or "auto" (tries traditional first, CUA as fallback)
        max_results: Maximum results for traditional scraper
        cua_timeout: Timeout in seconds for CUA scraper (default 45s for speed)
    
    Returns:
        Dictionary with results and metadata
    """
    start_time = time.time()
    results = {
        'query': query,
        'mode_requested': mode,
        'mode_executed': None,
        'products': [],
        'success': False,
        'runtime_seconds': 0,
        'error_message': None
    }
    
    try:
        if mode == "traditional":
            # Traditional scraper only
            products = run_traditional_scraper(query, max_results)
            results['mode_executed'] = 'traditional'
            results['products'] = products
            results['success'] = len(products) > 0
            
        elif mode == "cua":
            # CUA scraper only
            products = run_cua_scraper_with_timeout(query, cua_timeout, fast_mode=True)
            results['mode_executed'] = 'cua'
            results['products'] = products
            results['success'] = len(products) > 0
            
        elif mode == "auto":
            # Try traditional first, CUA as fast fallback
            print("🔄 Auto mode: Trying traditional scraper first...")
            products = run_traditional_scraper(query, max_results)
            
            if products and len(products) > 0:
                results['mode_executed'] = 'traditional'
                results['products'] = products
                results['success'] = True
                print("✅ Traditional scraper succeeded in auto mode")
            else:
                print("🔄 Traditional failed, falling back to FAST CUA scraper...")
                products = run_cua_scraper_with_timeout(query, cua_timeout, fast_mode=True)
                results['mode_executed'] = 'cua_fallback'
                results['products'] = products
                results['success'] = len(products) > 0
                if results['success']:
                    print("✅ CUA fallback scraper succeeded!")
                else:
                    print("❌ Both traditional and CUA scrapers failed")
                
        else:
            raise ValueError(f"Invalid mode: {mode}. Use 'traditional', 'cua', or 'auto'")
            
    except Exception as e:
        results['error_message'] = str(e)
        print(f"❌ Dual-mode search error: {e}")
    
    results['runtime_seconds'] = round(time.time() - start_time, 2)
    
    # Save results to DynamoDB if we have products
    if results['products'] and results['success']:
        saved_count = insert_products_to_dynamodb(results['products'])
        results['saved_to_db'] = saved_count
        print(f"📤 Saved {saved_count} products to DynamoDB")
    
    return results

