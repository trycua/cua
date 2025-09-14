# Shopping API - Natural Language Product Search

A powerful shopping tool that processes natural language queries like "find me the best tents under 100 dollars", searches multiple e-commerce sites, and stores results in DynamoDB.

## Features

- 🔍 **Natural Language Processing**: Parse queries like "cheap laptops under 500" or "top rated headphones between 50 and 200 dollars"
- 🛒 **Multi-Site Search**: Searches Amazon, Walmart, and eBay simultaneously
- 💾 **DynamoDB Integration**: Automatically stores search results with your existing pipeline
- 🤖 **AI Enhancement**: Optional OpenAI integration for better query understanding
- 📊 **Smart Filtering**: Price ranges, categories, and quality preferences
- 🔄 **RESTful API**: Easy integration with any frontend or service

## Quick Start

### 1. Environment Setup

Copy the example environment file and add your credentials:

```bash
cp .env.example .env
```

Edit `.env` with your credentials:
```bash
# Required for DynamoDB
AMAZON_API_KEY=your_aws_access_key_here
AMAZON_SECRET_KEY=your_aws_secret_key_here
AWS_REGION=ca-central-1

# Optional for enhanced AI processing
OPENAI_API_KEY=your_openai_api_key_here
```

### 2. Install Dependencies

```bash
pip install -r requirements.txt
```

### 3. Setup DynamoDB Tables

```bash
python data-pipeline/dynamodb/dynamo-setup.py
```

### 4. Start the API

```bash
python shopping_api.py
```

The API will start on `http://localhost:5000`

### 5. Test the Setup

```bash
python test_shopping_api.py
```

## API Endpoints

### POST /search
Main search endpoint for natural language queries.

**Request:**
```json
{
    "query": "find me the best tents under 100 dollars",
    "max_results": 15,
    "store_in_db": true
}
```

**Response:**
```json
{
    "query": "find me the best tents under 100 dollars",
    "parsed_query": {
        "search_terms": "tents",
        "min_price": null,
        "max_price": 100.0,
        "sort_by": "rating"
    },
    "products": [...],
    "total_found": 25,
    "filtered_count": 18,
    "returned_count": 15,
    "stored_in_db": 15
}
```

### GET /search_history
Get recent search results from DynamoDB.

### GET /search_by_category
Search by category and price range.

**Parameters:**
- `category`: camping, electronics, fashion, etc.
- `price_range`: 0-25, 25-50, 50-100, 100-200, 200+

### GET /health
Health check endpoint.

## Example Queries

The API understands natural language queries like:

- `"find me the best tents under 100 dollars"`
- `"cheap laptops under 500"`
- `"top rated headphones between 50 and 200 dollars"`
- `"outdoor camping gear under 75"`
- `"best coffee makers over 100"`

## Query Processing

The system automatically extracts:
- **Product type**: tents, laptops, headphones, etc.
- **Price constraints**: under X, between X and Y, over X
- **Quality preferences**: best, top rated, cheap
- **Categories**: camping, electronics, fashion, etc.

## DynamoDB Schema

Products are stored with the following structure:
- `product_id` (Hash Key): Unique identifier
- `product_name`: Product title
- `site_name`: amazon, walmart, ebay
- `current_price`: Decimal price
- `rating`: Product rating
- `category`: Product category
- `price_range`: Indexed price range
- `quality_score`: Calculated quality metric

## Architecture

```
User Query → AI Parser → Web Scraper → Price Filter → DynamoDB → API Response
```

1. **Query Processing**: Parse natural language with AI or regex fallback
2. **Web Scraping**: Search Amazon, Walmart, eBay in parallel
3. **Data Processing**: Clean, filter, and sort results
4. **Storage**: Store in DynamoDB using existing pipeline
5. **Response**: Return structured results

## Error Handling

- Graceful fallbacks when AI parsing fails
- Retry logic for web scraping
- Comprehensive error logging
- Health check endpoints

## Development

### Project Structure
```
├── shopping_api.py          # Main Flask API
├── shopping_scraper.py      # Web scraping module
├── ai_query_processor.py    # AI-powered query parsing
├── test_shopping_api.py     # Test suite
├── data-pipeline/           # Your existing DynamoDB code
└── requirements.txt         # Dependencies
```

### Testing
```bash
# Run the test suite
python test_shopping_api.py

# Manual testing with curl
curl -X POST http://localhost:5000/search \
  -H "Content-Type: application/json" \
  -d '{"query": "find me the best tents under 100 dollars"}'
```

## Troubleshooting

### Environment Variables Not Loading
- Ensure `.env` file exists in project root
- Check file permissions
- Verify variable names match exactly

### DynamoDB Connection Issues
- Verify AWS credentials are correct
- Check AWS region setting
- Ensure DynamoDB tables exist (run setup script)

### Web Scraping Failures
- Sites may block requests - this is normal
- Results will come from available sites
- Consider adding delays between requests

## Next Steps

1. **Frontend Integration**: Build a web UI for the API
2. **More Sites**: Add support for additional e-commerce sites
3. **Advanced AI**: Implement more sophisticated query understanding
4. **Caching**: Add Redis for faster repeated searches
5. **Analytics**: Track popular searches and trends
