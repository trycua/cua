#!/usr/bin/env python3
"""
Simple DynamoDB table creation without Spark dependencies
"""

import os
import boto3
import botocore.exceptions
from dotenv import load_dotenv

load_dotenv('.env.local')

def create_dynamodb_table(table_name: str):
    """Create a simple DynamoDB table"""
    try:
        # Get AWS credentials
        aws_key = os.getenv("AMAZON_API_KEY")
        aws_secret = os.getenv("AMAZON_SECRET_KEY")
        region = os.getenv("AWS_REGION", "ca-central-1")
        
        # Create session
        session = boto3.Session(
            aws_access_key_id=aws_key,
            aws_secret_access_key=aws_secret,
            region_name=region
        )
        
        dynamodb = session.resource('dynamodb')
        
        # Create table
        table = dynamodb.create_table(
            TableName=table_name,
            KeySchema=[
                {'AttributeName': 'product_id', 'KeyType': 'HASH'}
            ],
            AttributeDefinitions=[
                {'AttributeName': 'product_id', 'AttributeType': 'S'}
            ],
            BillingMode='PAY_PER_REQUEST'
        )
        
        print(f"Creating table '{table_name}'...")
        table.wait_until_exists()
        print(f"✅ Table '{table_name}' created successfully!")
        return True
        
    except botocore.exceptions.ClientError as e:
        if e.response.get('Error', {}).get('Code') == 'ResourceInUseException':
            print(f"✅ Table '{table_name}' already exists")
            return True
        else:
            print(f"❌ Error creating table '{table_name}': {e}")
            return False
    except Exception as e:
        print(f"❌ Unexpected error creating table: {e}")
        return False

def create_shopping_tables():
    """Create required shopping tables"""
    tables = [
        'shopping_products_unstructured',
        'selected_products_result'
    ]
    
    for table in tables:
        success = create_dynamodb_table(table)
        if not success:
            return False
    return True

if __name__ == "__main__":
    create_shopping_tables()
