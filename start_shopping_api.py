#!/usr/bin/env python3
"""
Startup script for the Shopping API
Handles environment setup and starts the API
"""

import sys
import subprocess
import os
from pathlib import Path

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
                sys.executable, '-m', 'pip', 'install', '--user'
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
        from data_pipeline.dynamodb.dynamo_setup import create_dynamodb_tables
        create_dynamodb_tables()
        print("✅ DynamoDB tables ready")
        return True
    except Exception as e:
        print(f"❌ DynamoDB setup failed: {e}")
        print("Please check your AWS credentials and try again")
        return False

def start_api():
    """Start the shopping API"""
    try:
        print("🚀 Starting Shopping API...")
        from shopping_api import app
        app.run(debug=True, host='0.0.0.0', port=5000)
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
    print("\n🎉 Starting API on http://localhost:5000")
    print("📋 Available endpoints:")
    print("  POST /search - Main search endpoint")
    print("  GET /search_history - Recent searches")
    print("  GET /health - Health check")
    print("\n💡 Example usage:")
    print('curl -X POST http://localhost:5000/search -H "Content-Type: application/json" -d \'{"query": "find me the best tents under 100 dollars"}\'')
    print("\nPress Ctrl+C to stop the server")
    
    start_api()

if __name__ == "__main__":
    main()
