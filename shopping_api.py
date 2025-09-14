from flask import Flask, request, jsonify
import os
from typing import Dict, List, Optional
import re
from datetime import datetime
import hashlib
from decimal import Decimal

# Import our modules
from shopping_scraper import ProductScraper, extract_price_filter, filter_products_by_price

# Try to import DynamoDB modules, fall back to mock if not available
try:
    import boto3
    from dotenv import load_dotenv
    load_dotenv()
    
    # Create DynamoDB connection directly
    aws_key = os.getenv("AMAZON_API_KEY")
    aws_secret = os.getenv("AMAZON_SECRET_KEY")
    region = os.getenv("AWS_REGION", "us-east-1")
    
    if aws_key and aws_secret:
        session = boto3.Session(
            aws_access_key_id=aws_key,
            aws_secret_access_key=aws_secret,
            region_name=region
        )
        dynamodb = session.resource('dynamodb')
        DYNAMODB_AVAILABLE = True
        print("✅ DynamoDB connection established")
    else:
        raise ImportError("AWS credentials not found")
        
except ImportError as e:
    print(f"⚠️ DynamoDB not available: {e}")
    DYNAMODB_AVAILABLE = False
    dynamodb = None

# DynamoDB processing functions
def convert_floats_to_decimal(obj):
    if isinstance(obj, float):
        return Decimal(str(obj))
    elif isinstance(obj, dict):
        return {k: convert_floats_to_decimal(v) for k, v in obj.items()}
    elif isinstance(obj, list):
        return [convert_floats_to_decimal(v) for v in obj]
    return obj

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
                rating_numeric = Decimal(rating_str.split()[0])
            elif 'stars' in rating_str:
                rating_numeric = Decimal(rating_str.replace('stars', '').strip())
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
        
        # Calculate quality score
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
    if not DYNAMODB_AVAILABLE or dynamodb is None:
        return 0
        
    table = dynamodb.Table('shopping_products_unstructured')
    
    success_count = 0
    error_count = 0
    
    for product in products:
        try:
            # Convert any remaining floats to Decimal
            product_item = convert_floats_to_decimal(product)
            table.put_item(Item=product_item)
            success_count += 1
            print(f"✅ Inserted: {product['product_name']}")
        except Exception as e:
            error_count += 1
            print(f"❌ Error inserting {product['product_name']}: {e}")
    
    print(f"\n📊 Results: {success_count} successful, {error_count} errors")
    return success_count

app = Flask(__name__)

class ShoppingQueryProcessor:
    def __init__(self):
        self.scraper = ProductScraper()
    
    def parse_query(self, query: str) -> Dict:
        """Parse natural language query to extract search terms and filters"""
        query_lower = query.lower()
        
        # Extract price filters
        min_price, max_price = extract_price_filter(query)
        
        # Extract product type/search terms
        # Remove price-related words to get clean search terms
        clean_query = re.sub(r'\b(under|over|between|and|\$\d+(?:\.\d{2})?)\b', '', query_lower)
        clean_query = re.sub(r'\bdollars?\b', '', clean_query)
        clean_query = re.sub(r'\s+', ' ', clean_query).strip()
        
        # Extract sorting preference
        sort_by = 'relevance'  # default
        if 'best' in query_lower or 'top' in query_lower:
            sort_by = 'rating'
        elif 'cheap' in query_lower or 'lowest price' in query_lower:
            sort_by = 'price_low'
        elif 'expensive' in query_lower or 'highest price' in query_lower:
            sort_by = 'price_high'
        
        return {
            'search_terms': clean_query,
            'min_price': min_price,
            'max_price': max_price,
            'sort_by': sort_by,
            'original_query': query
        }
    
    def sort_products(self, products: List[Dict], sort_by: str) -> List[Dict]:
        """Sort products based on criteria"""
        if sort_by == 'price_low':
            return sorted(products, key=lambda x: self._extract_price(x['price']))
        elif sort_by == 'price_high':
            return sorted(products, key=lambda x: self._extract_price(x['price']), reverse=True)
        elif sort_by == 'rating':
            return sorted(products, key=lambda x: self._extract_rating(x['rating']), reverse=True)
        else:
            return products  # Keep original order (relevance)
    
    def _extract_price(self, price_str: str) -> float:
        """Extract numeric price from price string"""
        try:
            return float(price_str.replace('$', '').replace(',', ''))
        except:
            return 0.0
    
    def _extract_rating(self, rating_str: str) -> float:
        """Extract numeric rating from rating string"""
        try:
            if 'out of' in rating_str:
                return float(rating_str.split()[0])
            elif 'stars' in rating_str:
                return float(rating_str.replace('stars', '').strip())
            else:
                return 0.0
        except:
            return 0.0

processor = ShoppingQueryProcessor()

@app.route('/search', methods=['POST'])
def search_products():
    """Main endpoint for natural language product search"""
    try:
        data = request.get_json()
        if not data or 'query' not in data:
            return jsonify({'error': 'Query parameter is required'}), 400
        
        query = data['query']
        max_results = data.get('max_results', 15)
        store_results = data.get('store_in_db', True)
        
        print(f"🔍 Processing query: {query}")
        
        # Parse the natural language query
        parsed_query = processor.parse_query(query)
        print(f"📝 Parsed query: {parsed_query}")
        
        # Search for products
        raw_products = processor.scraper.search_all_sites(
            parsed_query['search_terms'], 
            max_results_per_site=max_results//3 + 1
        )
        
        if not raw_products:
            return jsonify({
                'query': query,
                'parsed_query': parsed_query,
                'products': [],
                'message': 'No products found for your search'
            }), 200
        
        # Filter by price if specified
        filtered_products = filter_products_by_price(
            raw_products, 
            parsed_query['min_price'], 
            parsed_query['max_price']
        )
        
        # Sort products
        sorted_products = processor.sort_products(filtered_products, parsed_query['sort_by'])
        
        # Limit results
        final_products = sorted_products[:max_results]
        
        # Store in DynamoDB if requested
        stored_count = 0
        if store_results and final_products and DYNAMODB_AVAILABLE:
            try:
                processed_products = clean_and_process_data(final_products)
                stored_count = insert_products_to_dynamodb(processed_products)
                print(f"💾 Stored {stored_count} products in DynamoDB")
            except Exception as e:
                print(f"❌ Error storing in DynamoDB: {e}")
        elif store_results and not DYNAMODB_AVAILABLE:
            print("⚠️ DynamoDB not available, skipping storage")
        
        return jsonify({
            'query': query,
            'parsed_query': parsed_query,
            'products': final_products,
            'total_found': len(raw_products),
            'filtered_count': len(filtered_products),
            'returned_count': len(final_products),
            'stored_in_db': stored_count,
            'timestamp': datetime.now().isoformat()
        }), 200
        
    except Exception as e:
        print(f"❌ Error processing search: {e}")
        return jsonify({'error': str(e)}), 500

@app.route('/search_history', methods=['GET'])
def get_search_history():
    """Get recent searches from DynamoDB"""
    if not DYNAMODB_AVAILABLE or dynamodb is None:
        return jsonify({
            'error': 'DynamoDB not available',
            'recent_products': [],
            'count': 0
        }), 503
    
    try:
        table = dynamodb.Table('shopping_products_unstructured')
        
        # Get recent products (last 50)
        response = table.scan(
            Limit=50,
            ProjectionExpression='product_id, product_name, site_name, current_price, rating, last_updated'
        )
        
        products = response.get('Items', [])
        
        # Sort by last_updated
        products.sort(key=lambda x: x.get('last_updated', ''), reverse=True)
        
        return jsonify({
            'recent_products': products,
            'count': len(products)
        }), 200
        
    except Exception as e:
        print(f"❌ Error getting search history: {e}")
        return jsonify({'error': str(e)}), 500

@app.route('/search_by_category', methods=['GET'])
def search_by_category():
    """Search products by category and price range"""
    if not DYNAMODB_AVAILABLE or dynamodb is None:
        return jsonify({
            'error': 'DynamoDB not available',
            'category': request.args.get('category', 'general'),
            'price_range': request.args.get('price_range'),
            'products': [],
            'count': 0
        }), 503
    
    try:
        category = request.args.get('category', 'general')
        price_range = request.args.get('price_range')
        
        table = dynamodb.Table('shopping_products_unstructured')
        
        if price_range:
            # Use the GSI to search by category and price range
            response = table.query(
                IndexName='category-price-index',
                KeyConditionExpression='category = :cat AND price_range = :price',
                ExpressionAttributeValues={
                    ':cat': category,
                    ':price': price_range
                }
            )
        else:
            # Search by category only
            response = table.query(
                IndexName='category-price-index',
                KeyConditionExpression='category = :cat',
                ExpressionAttributeValues={
                    ':cat': category
                }
            )
        
        products = response.get('Items', [])
        
        return jsonify({
            'category': category,
            'price_range': price_range,
            'products': products,
            'count': len(products)
        }), 200
        
    except Exception as e:
        print(f"❌ Error searching by category: {e}")
        return jsonify({'error': str(e)}), 500

@app.route('/health', methods=['GET'])
def health_check():
    """Health check endpoint"""
    return jsonify({
        'status': 'healthy',
        'service': 'Shopping API',
        'timestamp': datetime.now().isoformat()
    }), 200

@app.route('/', methods=['GET'])
def home():
    """API documentation"""
    return jsonify({
        'service': 'Shopping API',
        'version': '1.0.0',
        'endpoints': {
            'POST /search': 'Search for products with natural language query',
            'GET /search_history': 'Get recent search results from database',
            'GET /search_by_category': 'Search by category and price range',
            'GET /health': 'Health check'
        },
        'example_queries': [
            'find me the best tents under 100 dollars',
            'cheap laptops under 500',
            'top rated headphones between 50 and 200 dollars',
            'outdoor camping gear under 75'
        ]
    }), 200

if __name__ == '__main__':
    print("🚀 Starting Shopping API...")
    print("📋 Available endpoints:")
    print("  POST /search - Main search endpoint")
    print("  GET /search_history - Recent searches")
    print("  GET /search_by_category - Category search")
    print("  GET /health - Health check")
    print("\n💡 Example usage:")
    print('  curl -X POST http://localhost:5000/search -H "Content-Type: application/json" -d \'{"query": "find me the best tents under 100 dollars"}\'')
    
    app.run(debug=True, host='0.0.0.0', port=5000)
