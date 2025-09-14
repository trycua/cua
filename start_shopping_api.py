#!/usr/bin/env python3
"""
Startup script for the Shopping API
Handles environment setup and starts the API
"""

import sys
import subprocess
import os
from pathlib import Path
from typing import Any
import boto3

sys.path.append("./data_pipeline/dynamodb")

from chooseData import choose_k_best_items

def check_and_install_dependencies():
    """Check if required packages are available, install if needed"""
    required_packages = ['flask', 'beautifulsoup4', 'requests', 'boto3', 'python-dotenv']
    
    missing_packages = []
    for package in required_packages:
        try:
            __import__(package.replace('-', '_'))
        except ImportError:
            missing_packages.append(package)
    
    if missing_packages:
        print(f"Installing missing packages: {', '.join(missing_packages)}")
        try:
            subprocess.check_call([
                sys.executable, '-m', 'pip', 'install'
            ] + missing_packages)
            print("✅ Dependencies installed successfully")
        except subprocess.CalledProcessError as e:
            print(f"❌ Failed to install dependencies: {e}")
            print("Please install manually:")
            print(f"python3 -m pip install --user {' '.join(missing_packages)}")
            return False
    
    return True

def check_environment():
    """Check if .env file exists and has required variables"""
    env_file = Path('.env')
    
    if not env_file.exists():
        print("❌ .env file not found")
        print("Please copy .env.example to .env and add your credentials:")
        print("cp .env.example .env")
        return False
    
    # Load and check environment variables
    from dotenv import load_dotenv
    load_dotenv()
    
    required_vars = ['AMAZON_API_KEY', 'AMAZON_SECRET_KEY']
    missing_vars = []
    
    for var in required_vars:
        if not os.getenv(var):
            missing_vars.append(var)
    
    if missing_vars:
        print(f"❌ Missing required environment variables: {', '.join(missing_vars)}")
        print("Please add these to your .env file")
        return False
    
    print("✅ Environment variables configured")
    return True

def setup_dynamodb():
    """Setup DynamoDB tables if needed"""
    try:
        print("🔧 Setting up DynamoDB tables...")
        from data_pipeline.dynamodb.simple_table_setup import create_shopping_tables
        create_shopping_tables()
        print("✅ DynamoDB tables ready")
        return True
    except Exception as e:
        print(f"❌ DynamoDB setup failed: {e}")
        print("Please check your AWS credentials and try again")
        return False
    
def clear_unstructured_products():
    dynamodb = boto3.resource('dynamodb', region_name=os.getenv('AWS_REGION', 'ca-central-1'))
    table = dynamodb.Table('shopping_products_unstructured')
    data = table.scan().get('Items', [])

    with table.batch_writer() as batch:
        for item in data:
            # Extract key names from key_schema
            key_names = [key['AttributeName'] for key in table.key_schema]
            key_dict = {key_name: item[key_name] for key_name in key_names if key_name in item}
            batch.delete_item(Key=key_dict)


def start_api():
    """Start the shopping API"""
    try:
        print("🚀 Starting Shopping API...")
        # Import Flask and create app here
        from flask import Flask, request, jsonify
        from robust_scraper import ProductScraper
        from shopping_api import clean_and_process_data, insert_products_to_dynamodb
        from query_interpreter import QueryInterpreter
        from decimal import Decimal
        import json
        
        def convert_decimals(obj):
            """Convert Decimal objects to float for JSON serialization"""
            if isinstance(obj, list):
                return [convert_decimals(item) for item in obj]
            elif isinstance(obj, dict):
                return {key: convert_decimals(value) for key, value in obj.items()}
            elif isinstance(obj, Decimal):
                return float(obj)
            else:
                return obj
        
        app = Flask(__name__)
        scraper = ProductScraper()
        query_interpreter = QueryInterpreter()
        setup_dynamodb()
        
        @app.route('/health', methods=['GET'])
        def health():
            return jsonify({'status': 'healthy', 'service': 'shopping_api'})
        
        @app.route('/search', methods=['POST'])
        def search():
            try:
                data = request.get_json()
                query = data.get('query', '')
                
                if not query:
                    return jsonify({'error': 'Query parameter is required'}), 400
                
                # Interpret the query using Claude
                interpreted_query = query_interpreter.interpret_query(query)
                print(f"🧠 Query interpretation: {interpreted_query}")
                
                # Generate optimized search terms
                search_terms = query_interpreter.generate_optimized_search_terms(interpreted_query)
                print(f"🔍 Optimized search terms: {search_terms}")
                
                # Get site recommendations
                recommended_sites = query_interpreter.get_site_recommendations(interpreted_query)
                print(f"🏪 Recommended sites: {recommended_sites}")
                
                # Search products using the first optimized search term (or original query if none)
                search_query = search_terms[0] if search_terms else query
                products = scraper.search_all_sites(search_query, max_results_per_site=5)
                
                # Clean and process data
                processed_products = clean_and_process_data(products)
                
                # Save to DynamoDB if available
                saved_count = 0
                if processed_products:
                    saved_count = insert_products_to_dynamodb(processed_products)

                # Save 3 best results to result table (judged by Cohere AI)
                final_products = choose_k_best_items(3)

                clear_unstructured_products()
                
                return jsonify({
                    'query': query,
                    'interpretation': interpreted_query,
                    'optimized_search_terms': search_terms,
                    'recommended_sites': recommended_sites,
                    'products_found': len(products),
                    'products_saved': saved_count,
                    'products': convert_decimals(final_products)
                })
                
            except Exception as e:
                import traceback
                print(f"❌ Search error: {e}")
                print(f"❌ Full traceback: {traceback.format_exc()}")
                return jsonify({'error': 'Internal server error'}), 500
        
        @app.route('/interpret', methods=['POST'])
        def interpret_query():
            """Interpret a search query using Claude Anthropic"""
            try:
                data = request.get_json()
                query = data.get('query', '')
                
                if not query:
                    return jsonify({'error': 'Query parameter is required'}), 400
                
                # Interpret the query
                interpreted_query = query_interpreter.interpret_query(query)
                search_terms = query_interpreter.generate_optimized_search_terms(interpreted_query)
                recommended_sites = query_interpreter.get_site_recommendations(interpreted_query)
                
                return jsonify({
                    'query': query,
                    'interpretation': interpreted_query,
                    'optimized_search_terms': search_terms,
                    'recommended_sites': recommended_sites
                })
                
            except Exception as e:
                print(f"❌ Query interpretation error: {e}")
                return jsonify({
                    'error': 'Query interpretation failed',
                    'details': str(e)
                }), 500
        
        @app.route('/evaluate', methods=['POST'])
        def evaluate_products():
            """Use Cohere AI to select the best products from DynamoDB using chooseData function"""
            try:
                from simple_choose_data import choose_best_k_ai
                
                # Run the original chooseData function (without Spark)
                result = choose_best_k_ai()
                
                return jsonify(result)
                
            except Exception as e:
                print(f"❌ Cohere evaluation error: {e}")
                return jsonify({
                    'error': 'Cohere evaluation failed',
                    'details': str(e)
                }), 500
        
        @app.route('/search_history', methods=['GET'])
        def search_history():
            return jsonify({'message': 'Search history endpoint - functionality removed'})
        
        @app.route('/search_by_category', methods=['GET'])
        def search_by_category():
            return jsonify({'message': 'Category search endpoint - functionality removed'})
        
        app.run(debug=True, host='0.0.0.0', port=5001)
    except Exception as e:
        print(f"❌ Failed to start API: {e}")
        return False

def main():
    """Main startup sequence"""
    print("🛒 Shopping API Startup")
    print("=" * 40)
    
    # Check dependencies
    if not check_and_install_dependencies():
        sys.exit(1)
    
    # Check environment
    if not check_environment():
        sys.exit(1)
    
    # Setup DynamoDB
    if not setup_dynamodb():
        print("⚠️ DynamoDB setup failed, but API can still run")
    
    # Start API
    print("\n🎉 Starting API on http://localhost:5001")
    print("📋 Available endpoints:")
    print("  POST /search - Main search endpoint")
    print("  GET /search_history - Recent searches")
    print("  GET /health - Health check")
    print("\n💡 Example usage:")
    print('curl -X POST http://localhost:5001/search -H "Content-Type: application/json" -d \'{"query": "find me the best tents under 100 dollars"}\'')
    print("\nPress Ctrl+C to stop the server")
    
    start_api()

if __name__ == "__main__":
    main()
