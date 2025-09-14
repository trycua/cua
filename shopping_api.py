#!/usr/bin/env python3
"""
Shopping API - Core functionality for product search and data processing
"""

import os
import time
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
                'timestamp': int(time.time()),
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

