import hashlib
from decimal import Decimal
from datetime import datetime
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

def convert_floats_to_decimal(obj):
    if isinstance(obj, float):
        return Decimal(str(obj))
    elif isinstance(obj, dict):
        return {k: convert_floats_to_decimal(v) for k, v in obj.items()}
    elif isinstance(obj, list):
        return [convert_floats_to_decimal(v) for v in obj]
    return obj

# REPLACE WITH ACTUAL WEB SCRAPED DATA
def scrape_sample_products():
    """Simulate scraped product data (replace with your CUA output)"""
    
    sample_products = [
        {
            "site_name": "amazon",
            "product_name": "Bialetti Moka Express 6-Cup",
            "product_url": "https://amazon.com/dp/B00004RG37",
            "price": "$34.99",
            "rating": "4.6 out of 5 stars",
            "review_count": "12,847 reviews",
            "description": "Traditional Italian stovetop espresso maker",
            "category": "coffee",
            "availability": "In Stock"
        },
        {
            "site_name": "amazon",
            "product_name": "De'Longhi Dedica Style EC685M",
            "product_url": "https://amazon.com/dp/B077JBQZPX",
            "price": "$149.95",
            "rating": "4.3 out of 5 stars", 
            "review_count": "2,341 reviews",
            "description": "Compact espresso machine with milk frother",
            "category": "coffee",
            "availability": "In Stock"
        },
        {
            "site_name": "walmart",
            "product_name": "Ninja Coffee Maker CM401",
            "product_url": "https://walmart.com/ip/12345",
            "price": "$89.99",
            "rating": "4.4 stars",
            "review_count": "1,563 reviews", 
            "description": "10-cup drip coffee maker with hot plate",
            "category": "coffee",
            "availability": "Available"
        }
    ]
    
    return sample_products

def clean_and_process_data(raw_products):
    """Clean the scraped data before inserting"""
    
    processed_products = []
    
    for product in raw_products:
        # Clean price - convert to Decimal directly
        price_str = product['price'].replace('$', '').replace(',', '')
        try:
            price_numeric = Decimal(price_str)
        except:
            price_numeric = Decimal('0.0')
        
        # Clean rating - convert to Decimal directly
        rating_str = product['rating']
        try:
            if 'out of' in rating_str:
                rating_numeric = Decimal(rating_str.split()[0])  # ✅ Use Decimal
            elif 'stars' in rating_str:
                rating_numeric = Decimal(rating_str.replace('stars', '').strip())  # ✅ Use Decimal
            else:
                rating_numeric = Decimal('0.0')
        except:
            rating_numeric = Decimal('0.0')
        
        # Clean review count
        review_str = product['review_count'].replace(',', '').replace('reviews', '').strip()
        try:
            review_count = int(review_str)
        except:
            review_count = 0
        
        # Generate product ID
        product_hash = hashlib.md5(f"{product['product_name']}{product['site_name']}".encode()).hexdigest()
        
        # Determine price range
        if price_numeric < 25:
            price_range = "0-25"
        elif price_numeric < 50:
            price_range = "25-50" 
        elif price_numeric < 100:
            price_range = "50-100"
        elif price_numeric < 200:
            price_range = "100-200"
        else:
            price_range = "200+"
        
        # Calculate quality score - use Decimal for calculation
        quality_score = (rating_numeric * 20) + Decimal(str(min(review_count / 10, 50))) + (Decimal('30') if 'stock' in product['availability'].lower() else Decimal('0'))
        
        processed_product = {
            'product_id': product_hash,
            'site_name': product['site_name'],
            'product_name': product['product_name'],
            'product_url': product['product_url'],
            'current_price': price_numeric,
            'rating': rating_numeric,
            'review_count': review_count,
            'description': product['description'],
            'category': product['category'],
            'price_range': price_range,
            'availability_status': 'in_stock' if 'stock' in product['availability'].lower() else 'unknown',
            'quality_score': quality_score,
            'last_updated': datetime.now().isoformat()
        }
        
        processed_products.append(processed_product)
    
    return processed_products

def insert_products_to_dynamodb(products):
    """Insert processed products into DynamoDB"""

    table = dynamodb.Table('shopping_products_unstructured')
    
    success_count = 0
    error_count = 0
    
    for product in products:
        try:
            # Convert any remaining floats to Decimal (just to be safe)
            product_item = convert_floats_to_decimal(product)
            table.put_item(Item=product_item)
            success_count += 1
            print(f"✅ Inserted: {product['product_name']}")
        except Exception as e:
            error_count += 1
            print(f"❌ Error inserting {product['product_name']}: {e}")
    
    print(f"\n📊 Results: {success_count} successful, {error_count} errors")
    return success_count

def ingest_data_pipeline():
    """Complete pipeline to get data into DynamoDB"""
    
    print("🚀 Starting data ingestion pipeline...\n")
    
    # Step 1: Get raw data (replace this with your CUA scraping)
    print("📥 Getting product data...")
    # REPLACE WITH ACTUAL SCRAPED DATA
    raw_products = scrape_sample_products()
    print(f"Found {len(raw_products)} raw products")
    
    # Step 2: Clean and process
    print("\n🧹 Cleaning and processing data...")
    processed_products = clean_and_process_data(raw_products)
    print(f"Processed {len(processed_products)} products")
    
    # Step 3: Insert into DynamoDB
    print("\n💾 Inserting into DynamoDB...")
    success_count = insert_products_to_dynamodb(processed_products)
    
    print(f"\n✅ Pipeline completed! {success_count} products in DynamoDB")
    
    return processed_products

# Run the ingestion pipeline
if __name__ == "__main__":
    processed_data = ingest_data_pipeline()