import copy
import cohere
from typing import Any
from cohere import ChatMessages, TextAssistantMessageResponseContentItem
import os
import boto3
# from pyspark.sql import SparkSession
from dotenv import load_dotenv

load_dotenv()

aws_key = os.getenv("AMAZON_API_KEY")
aws_secret= os.getenv("AMAZON_SECRET_KEY")
region = os.getenv("AWS_REGION", "ca-central-1")
cohere_key = os.getenv("COHERE_API_KEY")

# spark = SparkSession.builder.appName("ShoppingAgent").getOrCreate()
session = boto3.Session(
    aws_access_key_id=aws_key,
    aws_secret_access_key=aws_secret,
    region_name=region
)
dynamodb: Any = session.resource('dynamodb')
table = dynamodb.Table("shopping_products")

def choose_k_best_items(k: int = 1):
    items = []
    for item in table.scan()["Items"]:
        items.append(item)

    # Clean data
    # itemsForAi = copy.deepcopy([x for x in items if x["availability_status"] == "in_stock" or x["availability_status"] == "unknown"])
    itemsForAi = copy.deepcopy(items)
    for item in itemsForAi:
        for key in list(item.keys()):
            if key not in ["category", "current_price", "description", "price_range", "product_name", "quality_score", "rating", "review_count", "site_name"]:
                del item[key]

    system_prompt = f"""You are a shopping advisor. You will be given a list of {k} product(s) in JSON array format. 
    Each product has a category, current_price, description, price_range, product_name, quality_score, 
    rating, review_count, and site_name. Your task is to help the user find the best product based on
    affordability, quality, reviews, and reputation. You will do this by returning the indices of the best {k} product(s)
    in the JSON array, separated by commas and no spaces (e.g. "0,2,3" for the first, third, and fourth products).

    CRITICAL INSTRUCTIONS:
    1. ONLY return the indices of the best {k} products, separated by commas, in the JSON array. Do not provide explanations, commentary, or any other product details.
    2. Generally, the priority should be quality > reviews > affordability > reputation, but use your best judgement."""

    print("Item choices given to AI:")
    print(itemsForAi)
    print()

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
            "content": str(itemsForAi)
        }] # type: ignore
    )

    # Extract the LLM's response
    # TODO: Add error handling for non-integer responses
    text_response = response.message.content[0].text # type: ignore
    print("Overall response:", text_response)
    for index in text_response.split(","):
        index = index.strip()
        if not index.isdigit() or int(index) < 0 or int(index) >= len(items):
            print("Error: Cohere returned an invalid index:", index)
            return
    
        selected_index = int(index)
        print("Cohere responded with:", selected_index)
        print("This corresponds to the product:", items[selected_index])

        result_table = dynamodb.Table("selected_products_result")
        result_table.put_item(
            Item=items[selected_index]
        )

    # TODO: Use FastAPI to send result back to frontend

if __name__ == "__main__":
    choose_k_best_items()