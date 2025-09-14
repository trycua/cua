import copy
import json
import cohere
from typing import Any
from cohere import ChatMessages, TextAssistantMessageResponseContentItem
import os
import boto3
from pyspark.sql import SparkSession
from dotenv import load_dotenv

load_dotenv()

aws_key = os.getenv("AMAZON_API_KEY")
aws_secret= os.getenv("AMAZON_SECRET_KEY")
region = os.getenv("AWS_REGION", "ca-central-1")
cohere_key = os.getenv("COHERE_API_KEY")

spark = SparkSession.builder.appName("ShoppingAgent").getOrCreate()
session = boto3.Session(
    aws_access_key_id=aws_key,
    aws_secret_access_key=aws_secret,
    region_name=region
)
dynamodb: Any = session.resource('dynamodb')
table = dynamodb.Table("shopping_products_unstructured")

system_prompt = """You are a data structuring AI. You will be given an unstructured list of items that
were scraped from an e-commerce site. Your task is to generate a structured JSON array of these items 
where each item has the following fields: category (string), current price (number), description (string),
price_range (string in the form of a-b where a and b are the bounds of the range), product_name (string),
product_url (string), rating (number between 0 and 5), review_count (number), site_name (string). If
you cannot find a value for a field, use null as the value."""

def structure_data():
    for item in table.scan()["Items"]:
        co = cohere.ClientV2(cohere_key)
        response = co.chat(
            model="command-a-03-2025",
            messages=[
            {
                "role": "system", 
                "content": system_prompt
            },
            {
                "role": "user",
                "content": str(item)
            }], # type: ignore
            response_format={
                "type": "json_object",
                "schema:": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "category": {"type": "string"},
                            "current_price": {"type": ["number", "null"]},
                            "description": {"type": "string"},
                            "price_range": {"type": "string"},
                            "product_name": {"type": "string"},
                            "product_url": {"type": "string"},
                            "rating": {"type": ["number", "null"]},
                            "review_count": {"type": ["number", "null"]},
                            "site_name": {"type": "string"}
                        },
                        "required": ["current_price, product_name, rating"]
                    }
                }
            } # type: ignore
        )

        items = json.loads(response.message.content[0].text) # type: ignore
        print(items)

        result_table = dynamodb.Table("shopping_products")
        for item in items:
            result_table.put_item(
                Item=item
            )

if __name__ == "__main__":
    structure_data()
