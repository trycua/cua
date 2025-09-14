#!/usr/bin/env python3
"""
Test DynamoDB write functionality directly
"""

import os
import boto3
import time
from decimal import Decimal
from dotenv import load_dotenv

load_dotenv('.env.local')

# Get credentials
aws_key = os.getenv('AMAZON_API_KEY')
aws_secret = os.getenv('AMAZON_SECRET_KEY')
aws_region = os.getenv('AWS_REGION', 'ca-central-1')

print(f"AWS Key: {'✅ Set' if aws_key else '❌ Missing'}")
print(f"AWS Secret: {'✅ Set' if aws_secret else '❌ Missing'}")
print(f"AWS Region: {aws_region}")

try:
    # Create DynamoDB resource
    dynamodb = boto3.resource(
        'dynamodb',
        region_name=aws_region,
        aws_access_key_id=aws_key,
        aws_secret_access_key=aws_secret
    )
    
    # Get table
    table = dynamodb.Table('shopping_products_unstructured')
    
    # Test write with simple data (using Decimal for DynamoDB compatibility)
    test_item = {
        'product_id': f'test_{int(time.time())}',
        'product_name': 'Test Product',
        'price': Decimal('29.99'),
        'price_str': '$29.99',
        'rating': Decimal('4.5'),
        'rating_str': '4.5 stars',
        'site_name': 'test',
        'product_url': 'https://example.com',
        'description': 'Test product description',
        'category': 'test',
        'availability': 'Available',
        'review_count': '100',
        'timestamp': int(time.time()),
        'extraction_type': 'test_write'
    }
    
    print(f"\n🔄 Attempting to write test item...")
    print(f"Item: {test_item}")
    
    # Try to write
    response = table.put_item(Item=test_item)
    print(f"✅ Successfully wrote to DynamoDB!")
    print(f"Response: {response}")
    
    # Try to read it back
    print(f"\n🔄 Reading item back...")
    response = table.get_item(Key={'product_id': test_item['product_id']})
    if 'Item' in response:
        print(f"✅ Successfully read back: {response['Item']}")
    else:
        print(f"❌ Item not found in table")
        
except Exception as e:
    print(f"❌ DynamoDB error: {e}")
    import traceback
    traceback.print_exc()
