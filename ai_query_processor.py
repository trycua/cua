import os
import re
from typing import Dict, List, Optional, Tuple
from openai import OpenAI
from dotenv import load_dotenv

load_dotenv()

class AIQueryProcessor:
    def __init__(self):
        self.client = OpenAI(api_key=os.getenv('OPENAI_API_KEY'))
    
    def enhance_query_parsing(self, query: str) -> Dict:
        """Use AI to better understand and parse natural language queries"""
        try:
            prompt = f"""
            Parse this shopping query and extract structured information:
            Query: "{query}"
            
            Extract:
            1. Product type/category (e.g., "tents", "laptops", "headphones")
            2. Price constraints (min_price, max_price as numbers or null)
            3. Quality preferences (e.g., "best", "top rated", "cheap")
            4. Brand preferences if mentioned
            5. Specific features mentioned
            6. Clean search terms for web scraping
            
            Return as JSON:
            {{
                "product_type": "string",
                "search_terms": "cleaned search terms",
                "min_price": number or null,
                "max_price": number or null,
                "quality_preference": "best|cheap|any",
                "brand": "string or null",
                "features": ["list", "of", "features"],
                "category": "general category"
            }}
            """
            
            response = self.client.chat.completions.create(
                model="gpt-3.5-turbo",
                messages=[{"role": "user", "content": prompt}],
                temperature=0.1
            )
            
            import json
            result = json.loads(response.choices[0].message.content)
            return result
            
        except Exception as e:
            print(f"AI parsing failed, using fallback: {e}")
            return self._fallback_parsing(query)
    
    def _fallback_parsing(self, query: str) -> Dict:
        """Fallback parsing without AI"""
        query_lower = query.lower()
        
        # Extract price constraints
        min_price, max_price = self._extract_price_constraints(query)
        
        # Extract product type
        product_type = self._extract_product_type(query)
        
        # Clean search terms
        search_terms = re.sub(r'\b(under|over|between|and|\$\d+(?:\.\d{2})?|dollars?|best|top|cheap)\b', '', query_lower)
        search_terms = re.sub(r'\s+', ' ', search_terms).strip()
        
        # Quality preference
        quality_preference = "any"
        if any(word in query_lower for word in ["best", "top", "highest rated"]):
            quality_preference = "best"
        elif any(word in query_lower for word in ["cheap", "cheapest", "lowest price"]):
            quality_preference = "cheap"
        
        return {
            "product_type": product_type,
            "search_terms": search_terms,
            "min_price": min_price,
            "max_price": max_price,
            "quality_preference": quality_preference,
            "brand": None,
            "features": [],
            "category": self._categorize_product(product_type)
        }
    
    def _extract_price_constraints(self, query: str) -> Tuple[Optional[float], Optional[float]]:
        """Extract price constraints from query"""
        query_lower = query.lower()
        
        # Under X
        under_match = re.search(r'under\s+\$?(\d+(?:\.\d{2})?)', query_lower)
        if under_match:
            return None, float(under_match.group(1))
        
        # Between X and Y
        between_match = re.search(r'between\s+\$?(\d+(?:\.\d{2})?)\s+and\s+\$?(\d+(?:\.\d{2})?)', query_lower)
        if between_match:
            return float(between_match.group(1)), float(between_match.group(2))
        
        # Over X
        over_match = re.search(r'over\s+\$?(\d+(?:\.\d{2})?)', query_lower)
        if over_match:
            return float(over_match.group(1)), None
        
        return None, None
    
    def _extract_product_type(self, query: str) -> str:
        """Extract the main product type from query"""
        query_lower = query.lower()
        
        # Common product types
        product_keywords = [
            'tent', 'laptop', 'computer', 'phone', 'headphone', 'earbuds',
            'book', 'shirt', 'shoes', 'jacket', 'pants', 'dress',
            'chair', 'table', 'bed', 'sofa', 'desk',
            'camera', 'watch', 'tablet', 'speaker', 'mouse', 'keyboard',
            'backpack', 'bag', 'wallet', 'sunglasses',
            'tool', 'drill', 'hammer', 'screwdriver',
            'bike', 'helmet', 'gloves'
        ]
        
        for keyword in product_keywords:
            if keyword in query_lower:
                return keyword
        
        # If no specific product found, return first meaningful word
        words = query_lower.split()
        for word in words:
            if len(word) > 3 and word not in ['find', 'best', 'good', 'cheap', 'under', 'over']:
                return word
        
        return 'product'
    
    def _categorize_product(self, product_type: str) -> str:
        """Categorize product type into broader categories"""
        categories = {
            'electronics': ['laptop', 'computer', 'phone', 'tablet', 'camera', 'headphone', 'earbuds', 'speaker', 'mouse', 'keyboard', 'watch'],
            'clothing': ['shirt', 'shoes', 'jacket', 'pants', 'dress', 'hat', 'gloves'],
            'home': ['chair', 'table', 'bed', 'sofa', 'desk', 'lamp'],
            'outdoor': ['tent', 'backpack', 'bike', 'helmet'],
            'tools': ['drill', 'hammer', 'screwdriver', 'tool'],
            'books': ['book', 'novel', 'textbook'],
            'accessories': ['bag', 'wallet', 'sunglasses']
        }
        
        for category, keywords in categories.items():
            if product_type.lower() in keywords:
                return category
        
        return 'general'
    
    def generate_search_suggestions(self, query: str, found_products: List[Dict]) -> List[str]:
        """Generate alternative search suggestions based on results"""
        if not found_products:
            parsed = self.enhance_query_parsing(query)
            product_type = parsed.get('product_type', 'products')
            
            suggestions = [
                f"Try searching for '{product_type}' with different price ranges",
                f"Search for '{product_type} alternatives'",
                f"Look for '{product_type} reviews' to find popular options"
            ]
            return suggestions
        
        # If we have products, suggest refinements
        categories = set(p.get('category', 'general') for p in found_products)
        sites = set(p.get('site_name', '') for p in found_products)
        
        suggestions = []
        if len(categories) > 1:
            suggestions.append(f"Filter by category: {', '.join(categories)}")
        if len(sites) > 1:
            suggestions.append(f"Search specific sites: {', '.join(sites)}")
        
        return suggestions[:3]
