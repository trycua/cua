#!/usr/bin/env python3
"""
Query Interpreter using Claude Anthropic to extract user intent from search queries
"""

import os
import json
from typing import Dict, List, Optional, Any
from dotenv import load_dotenv
import anthropic

load_dotenv()

class QueryInterpreter:
    def __init__(self):
        """Initialize the Claude Anthropic client"""
        self.client = anthropic.Anthropic(
            api_key=os.getenv("ANTHROPIC_API_KEY")
        )
        
    def interpret_query(self, query: str) -> Dict[str, Any]:
        """
        Interpret a search query to extract user intent and preferences
        
        Args:
            query: The user's search query
            
        Returns:
            Dictionary containing extracted information about the query
        """
        
        system_prompt = """You are a shopping query interpreter. Your job is to analyze user search queries and extract structured information about what they're looking for.

Extract the following information from the query and return it as a JSON object:

{
    "product_category": "string - main product category (e.g., 'electronics', 'clothing', 'toys', 'home')",
    "product_type": "string - specific product type (e.g., 'headphones', 'teddy bear', 'laptop')",
    "brand_preferences": ["array of preferred brands if mentioned"],
    "price_range": {
        "min": "number or null - minimum price if specified",
        "max": "number or null - maximum price if specified",
        "currency": "string - currency symbol if mentioned (default: '$')"
    },
    "quality_indicators": ["array of quality terms like 'best', 'premium', 'high-quality', 'cheap', 'budget']",
    "specific_features": ["array of specific features mentioned"],
    "use_case": "string - intended use (e.g., 'gaming', 'work', 'gift', 'travel')",
    "target_audience": "string - who it's for (e.g., 'kids', 'adults', 'professionals')",
    "urgency": "string - urgency level ('low', 'medium', 'high')",
    "quantity": "number - how many items needed (default: 1)",
    "preferred_sites": ["array of preferred shopping sites if mentioned"],
    "color_preferences": ["array of preferred colors if mentioned"],
    "size_preferences": ["array of size preferences if mentioned"],
    "search_intent": "string - overall intent ('purchase', 'compare', 'research', 'gift')",
    "keywords": ["array of key search terms to use"]
}

Be precise and only extract information that is explicitly mentioned or strongly implied in the query. Use null for missing information."""

        try:
            response = self.client.messages.create(
                model="claude-sonnet-4-20250514",
                max_tokens=1000,
                temperature=0.1,
                system=system_prompt,
                messages=[
                    {
                        "role": "user",
                        "content": f"Analyze this search query: '{query}'"
                    }
                ]
            )
            
            # Extract the JSON from the response
            try:
                response_text = response.content[0].text
            except (AttributeError, IndexError):
                response_text = str(response.content)
            
            # Try to parse the JSON response
            try:
                interpreted_data = json.loads(response_text)
                return interpreted_data
            except json.JSONDecodeError:
                # If JSON parsing fails, try to extract JSON from the text
                import re
                json_match = re.search(r'\{.*\}', response_text, re.DOTALL)
                if json_match:
                    interpreted_data = json.loads(json_match.group())
                    return interpreted_data
                else:
                    # Fallback: return basic interpretation
                    return self._fallback_interpretation(query)
                    
        except Exception as e:
            print(f"❌ Query interpretation error: {e}")
            return self._fallback_interpretation(query)
    
    def _fallback_interpretation(self, query: str) -> Dict[str, Any]:
        """Fallback interpretation when Claude API fails"""
        return {
            "product_category": "general",
            "product_type": query.lower(),
            "brand_preferences": [],
            "price_range": {"min": None, "max": None, "currency": "$"},
            "quality_indicators": [],
            "specific_features": [],
            "use_case": None,
            "target_audience": None,
            "urgency": "medium",
            "quantity": 1,
            "preferred_sites": [],
            "color_preferences": [],
            "size_preferences": [],
            "search_intent": "purchase",
            "keywords": query.split()
        }
    
    def generate_optimized_search_terms(self, interpreted_query: Dict[str, Any]) -> List[str]:
        """
        Generate optimized search terms based on the interpreted query
        
        Args:
            interpreted_query: The result from interpret_query()
            
        Returns:
            List of optimized search terms
        """
        search_terms = []
        
        # Base product type
        if interpreted_query.get("product_type"):
            search_terms.append(interpreted_query["product_type"])
        
        # Add brand preferences
        for brand in interpreted_query.get("brand_preferences", []):
            search_terms.append(f"{brand} {interpreted_query.get('product_type', '')}")
        
        # Add quality indicators
        for quality in interpreted_query.get("quality_indicators", []):
            search_terms.append(f"{quality} {interpreted_query.get('product_type', '')}")
        
        # Add specific features
        for feature in interpreted_query.get("specific_features", []):
            search_terms.append(f"{interpreted_query.get('product_type', '')} {feature}")
        
        # Add use case context
        if interpreted_query.get("use_case"):
            search_terms.append(f"{interpreted_query.get('product_type', '')} for {interpreted_query['use_case']}")
        
        # Add target audience context
        if interpreted_query.get("target_audience"):
            search_terms.append(f"{interpreted_query.get('product_type', '')} for {interpreted_query['target_audience']}")
        
        # Add color preferences
        for color in interpreted_query.get("color_preferences", []):
            search_terms.append(f"{color} {interpreted_query.get('product_type', '')}")
        
        # Remove duplicates and empty terms
        search_terms = list(set([term.strip() for term in search_terms if term.strip()]))
        
        # If no specific terms generated, use keywords
        if not search_terms:
            search_terms = interpreted_query.get("keywords", [interpreted_query.get("product_type", "products")])
        
        return search_terms[:5]  # Limit to top 5 search terms
    
    def get_site_recommendations(self, interpreted_query: Dict[str, Any]) -> List[str]:
        """
        Recommend which sites to prioritize based on the query
        
        Args:
            interpreted_query: The result from interpret_query()
            
        Returns:
            List of recommended sites in priority order
        """
        category = interpreted_query.get("product_category", "general")
        product_type = interpreted_query.get("product_type", "")
        price_range = interpreted_query.get("price_range", {})
        
        # Default site priority
        sites = ["amazon", "walmart", "ebay"]
        
        # Adjust based on category
        if category == "electronics":
            sites = ["amazon", "best_buy", "newegg", "walmart"]
        elif category == "clothing":
            sites = ["amazon", "walmart", "target"]
        elif category == "toys":
            sites = ["amazon", "walmart", "target"]
        elif category == "home":
            sites = ["amazon", "walmart", "home_depot", "target"]
        
        # Adjust based on price preferences
        if price_range.get("max") and price_range["max"] < 50:
            # Budget shopping - prioritize discount retailers
            sites = ["walmart", "amazon", "ebay"]
        elif price_range.get("min") and price_range["min"] > 100:
            # Premium shopping - prioritize quality retailers
            sites = ["amazon", "best_buy", "target"]
        
        return sites

def test_query_interpreter():
    """Test the query interpreter with sample queries"""
    interpreter = QueryInterpreter()
    
    test_queries = [
        "find me the best wireless headphones under $100",
        "cheap teddy bears for kids",
        "premium gaming laptop with RTX 4080",
        "red running shoes size 10",
        "bluetooth speakers for outdoor parties",
        "organic baby food",
        "Christmas gifts under $50"
    ]
    
    print("🧠 Testing Query Interpreter")
    print("=" * 50)
    
    for query in test_queries:
        print(f"\n📝 Query: '{query}'")
        result = interpreter.interpret_query(query)
        print(f"🎯 Interpretation: {json.dumps(result, indent=2)}")
        
        search_terms = interpreter.generate_optimized_search_terms(result)
        print(f"🔍 Optimized search terms: {search_terms}")
        
        sites = interpreter.get_site_recommendations(result)
        print(f"🏪 Recommended sites: {sites}")

# Main function for easy import and use
def extract_search_meaning(query: str) -> Dict[str, Any]:
    """
    Simple function to extract meaning from a search query
    
    Args:
        query: The user's search query string
        
    Returns:
        Dictionary containing extracted search intent and parameters
    """
    interpreter = QueryInterpreter()
    return interpreter.interpret_query(query)

def get_search_recommendations(query: str) -> Dict[str, Any]:
    """
    Get complete search recommendations including interpreted query, 
    optimized search terms, and site recommendations
    
    Args:
        query: The user's search query string
        
    Returns:
        Dictionary containing:
        - interpreted_query: Full interpretation results
        - search_terms: Optimized search terms
        - recommended_sites: Priority-ordered site list
    """
    interpreter = QueryInterpreter()
    interpreted = interpreter.interpret_query(query)
    
    return {
        "interpreted_query": interpreted,
        "search_terms": interpreter.generate_optimized_search_terms(interpreted),
        "recommended_sites": interpreter.get_site_recommendations(interpreted)
    }

if __name__ == "__main__":
    test_query_interpreter()
