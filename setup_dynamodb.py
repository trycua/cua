#!/usr/bin/env python3
"""
Simplified DynamoDB setup without PySpark dependency
"""

import boto3
import botocore.exceptions
import os
from dotenv import load_dotenv

# Load environment variables
load_dotenv()

def setup_dynamodb():
    """Create DynamoDB table for shopping products"""
    
    # Get AWS credentials
    aws_key = os.getenv("AMAZON_API_KEY")
    aws_secret = os.getenv("AMAZON_SECRET_KEY")
    region = os.getenv("AWS_REGION", "us-east-1")
    
    if not aws_key or not aws_secret:
        print("❌ AWS credentials not found in .env file")
        print("Please ensure AMAZON_API_KEY and AMAZON_SECRET_KEY are set")
        return False
    
    print(f"🔧 Setting up DynamoDB in region: {region}")
    
    # Create session
    session = boto3.Session(
        aws_access_key_id=aws_key,
        aws_secret_access_key=aws_secret,
        region_name=region
    )
    
    dynamodb = session.resource('dynamodb')
    
    try:
        table_name = 'shopping_products_unstructured'
        
        print(f"📝 Creating table: {table_name}")
        
        # Create products table
        table = dynamodb.create_table(
            TableName=table_name,
            KeySchema=[
                {'AttributeName': 'product_id', 'KeyType': 'HASH'}
            ],
            AttributeDefinitions=[
                {'AttributeName': 'product_id', 'AttributeType': 'S'},
                {'AttributeName': 'category', 'AttributeType': 'S'},
                {'AttributeName': 'price_range', 'AttributeType': 'S'}
            ],
            GlobalSecondaryIndexes=[
                {
                    'IndexName': 'category-price-index',
                    'KeySchema': [
                        {'AttributeName': 'category', 'KeyType': 'HASH'},
                        {'AttributeName': 'price_range', 'KeyType': 'RANGE'}
                    ],
                    'Projection': {'ProjectionType': 'ALL'},
                }
            ],
            BillingMode='PAY_PER_REQUEST'
        )
        
        print("⏳ Waiting for table to be created...")
        table.wait_until_exists()
        print("✅ Table created successfully!")
        
        return True
        
    except botocore.exceptions.ClientError as e:
        error_code = e.response.get('Error', {}).get('Code')
        if error_code == 'ResourceInUseException':
            print("✅ Table already exists!")
            return True
        else:
            print(f"❌ Error creating table: {e}")
            return False
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        return False

def test_connection():
    """Test DynamoDB connection"""
    
    aws_key = os.getenv("AMAZON_API_KEY")
    aws_secret = os.getenv("AMAZON_SECRET_KEY")
    region = os.getenv("AWS_REGION", "us-east-1")
    
    try:
        session = boto3.Session(
            aws_access_key_id=aws_key,
            aws_secret_access_key=aws_secret,
            region_name=region
        )
        
        dynamodb = session.resource('dynamodb')
        
        # List tables to test connection
        client = session.client('dynamodb')
        response = client.list_tables()
        
        print(f"🔗 Connected to DynamoDB in {region}")
        print(f"📊 Found {len(response['TableNames'])} tables")
        
        if 'shopping_products_unstructured' in response['TableNames']:
            print("✅ shopping_products_unstructured table exists")
        else:
            print("⚠️ shopping_products_unstructured table not found")
        
        return True
        
    except Exception as e:
        print(f"❌ Connection failed: {e}")
        return False

if __name__ == "__main__":
    print("🚀 DynamoDB Setup")
    print("=" * 40)
    
    # Test connection first
    if test_connection():
        # Setup table
        setup_dynamodb()
    else:
        print("Please check your AWS credentials and try again")
