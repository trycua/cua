# SETUP: RUN ONCE
import botocore.exceptions
import os
from typing import Any
import boto3
from pyspark.sql import SparkSession
from dotenv import load_dotenv

load_dotenv()

aws_key = os.getenv("AMAZON_API_KEY")
aws_secret= os.getenv("AMAZON_SECRET_KEY")
region = os.getenv("AWS_REGION", "ca-central-1")  

spark = SparkSession.builder.appName("ShoppingAgent").getOrCreate()

session = boto3.Session(
    aws_access_key_id=aws_key,
    aws_secret_access_key=aws_secret,
    region_name=region
)

dynamodb: Any = session.resource('dynamodb')  

def create_dynamodb_tables():

    try:
        table_name = 'shopping_products_unstructured'
          
        # Products table
        products_table = dynamodb.create_table(
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
        
        products_table.wait_until_exists() 
        print("Tables created successfully!")
        return True
        
    except botocore.exceptions.ClientError as e:
        if e.response.get('Error', {}).get('Code') == 'ResourceInUseException':
            print("Tables already exist")
            return True
        else:
            print(f"Error creating tables: {e}")
            return False

# Run this first
create_dynamodb_tables()
