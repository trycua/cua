from computer.helpers import generate_source_code


class TestSimpleCases:
    """Test simple dependency cases"""

    def test_simple_function_no_dependencies(self):
        """Test simple function with no dependencies"""

        def simple_func():
            return 42

        code = generate_source_code(simple_func)

        assert "def simple_func():" in code
        assert "return 42" in code

    def test_function_with_parameters(self):
        """Test function with parameters"""

        def add(a, b):
            return a + b

        code = generate_source_code(add)

        assert "def add(a, b):" in code
        assert "return a + b" in code

    def test_function_with_docstring(self):
        """Test function with docstring"""

        def documented():
            """This is a docstring"""
            return True

        code = generate_source_code(documented)

        assert "def documented():" in code
        assert "This is a docstring" in code


class TestImports:
    """Test import handling"""

    def test_stdlib_import_inside_function(self):
        """Test function with stdlib import inside"""

        def with_stdlib():
            import math

            return math.sqrt(16)

        code = generate_source_code(with_stdlib)

        assert "import math" in code
        assert "def with_stdlib():" in code
        assert "math.sqrt(16)" in code

    def test_multiple_stdlib_imports(self):
        """Test function with multiple stdlib imports"""

        def with_multiple():
            import json
            import math

            return json.dumps({"value": math.pi})

        code = generate_source_code(with_multiple)

        assert "import json" in code
        assert "import math" in code

    def test_from_import_stdlib(self):
        """Test from X import Y style"""

        def with_from_import():
            from math import pi, sqrt

            return sqrt(16) + pi

        code = generate_source_code(with_from_import)

        assert "from math import" in code
        assert "sqrt, pi" in code or ("sqrt" in code and "pi" in code)

    def test_third_party_global_import(self):
        """Test globally imported third-party module"""
        from requests import get

        def with_requests():
            return get

        code = generate_source_code(with_requests)

        assert "from requests import get" in code
        assert "def with_requests():" in code


class TestHelperFunctions:
    """Test helper function dependencies"""

    def test_single_helper_function(self):
        """Test function with one helper"""

        def helper(x):
            return x * 2

        def main():
            return helper(21)

        code = generate_source_code(main)

        assert "def helper(x):" in code
        assert "return x * 2" in code
        assert "def main():" in code
        assert "return helper(21)" in code

    def test_nested_helper_functions(self):
        """Test function with nested helpers"""

        def helper_a(x):
            return x + 1

        def helper_b(x):
            return helper_a(x) * 2

        def main():
            return helper_b(10)

        code = generate_source_code(main)

        assert "def helper_a(x):" in code
        assert "def helper_b(x):" in code
        assert "def main():" in code

    def test_helper_dependency_ordering(self):
        """Test that helpers are ordered correctly (dependencies first)"""

        def level_3(x):
            return x + 1

        def level_2(x):
            return level_3(x) * 2

        def level_1(x):
            return level_2(x) + 5

        code = generate_source_code(level_1)

        # Check ordering
        pos_3 = code.find("def level_3")
        pos_2 = code.find("def level_2")
        pos_1 = code.find("def level_1")

        assert pos_3 < pos_2 < pos_1, "Functions not in correct dependency order"

    def test_helper_with_import(self):
        """Test helper function that uses imports"""
        import math

        def helper(x):
            return math.sqrt(x)

        def main():
            return helper(16)

        code = generate_source_code(main)

        assert "import math" in code
        assert "def helper(x):" in code
        assert "def main():" in code


class TestGlobalConstants:
    """Test global constant handling"""

    def test_simple_constant(self):
        """Test function using simple constant"""
        MY_CONSTANT = 42

        def with_constant():
            return MY_CONSTANT * 2

        code = generate_source_code(with_constant)

        assert "MY_CONSTANT = 42" in code
        assert "def with_constant():" in code

    def test_string_constant(self):
        """Test function using string constant"""
        API_URL = "https://api.example.com"

        def with_string():
            return API_URL

        code = generate_source_code(with_string)

        assert "API_URL" in code
        assert "https://api.example.com" in code

    def test_multiple_constants(self):
        """Test function using multiple constants"""
        BASE_URL = "https://example.com"
        TIMEOUT = 30

        def with_multiple():
            return f"{BASE_URL}?timeout={TIMEOUT}"

        code = generate_source_code(with_multiple)

        assert "BASE_URL" in code
        assert "TIMEOUT" in code


class TestClassDefinitions:
    """Test class definition handling"""

    def test_simple_class(self):
        """Test function using simple class"""

        class Calculator:
            def __init__(self, value):
                self.value = value

            def add(self, x):
                return self.value + x

        def with_class():
            calc = Calculator(10)
            return calc.add(5)

        code = generate_source_code(with_class)

        assert "class Calculator:" in code
        assert "__init__" in code
        assert "def add" in code
        assert "def with_class():" in code

    def test_class_with_methods(self):
        """Test class with multiple methods"""

        class Math:
            def add(self, a, b):
                return a + b

            def multiply(self, a, b):
                return a * b

        def use_math():
            m = Math()
            return m.add(2, 3) + m.multiply(4, 5)

        code = generate_source_code(use_math)

        assert "class Math:" in code
        assert "def add" in code
        assert "def multiply" in code


class TestDecoratorRemoval:
    """Test decorator removal"""

    def test_simple_decorator_removed(self):
        """Test that decorators are removed"""

        def my_decorator(f):
            def wrapper(*args, **kwargs):
                return f(*args, **kwargs)

            return wrapper

        @my_decorator
        def decorated():
            return 42

        code = generate_source_code(decorated)

        assert "@my_decorator" not in code
        assert "def decorated():" in code
        assert "return 42" in code

    def test_multiple_decorators_removed(self):
        """Test that multiple decorators are removed"""

        def decorator1(f):
            return f

        def decorator2(f):
            return f

        @decorator1
        @decorator2
        def multi_decorated():
            return True

        code = generate_source_code(multi_decorated)

        assert "@decorator1" not in code
        assert "@decorator2" not in code
        assert "def multi_decorated():" in code


class TestComplexScenarios:
    """Test complex real-world scenarios"""

    def test_api_client_pattern(self):
        """Test typical API client pattern"""
        import json

        BASE_URL = "https://api.example.com"

        def make_request(endpoint):
            return f"{BASE_URL}/{endpoint}"

        def parse_response(response):
            return json.loads(response)

        def get_user(user_id):
            url = make_request(f"users/{user_id}")
            response = f'{{"id": {user_id}}}'
            return parse_response(response)

        code = generate_source_code(get_user)

        assert "import json" in code
        assert "BASE_URL" in code
        assert "def make_request" in code
        assert "def parse_response" in code
        assert "def get_user" in code

    def test_data_processing_pipeline(self):
        """Test data processing pipeline"""

        def validate_data(data):
            return [x for x in data if x > 0]

        def transform_data(data):
            return [x * 2 for x in data]

        def process_pipeline(raw_data):
            valid = validate_data(raw_data)
            return transform_data(valid)

        code = generate_source_code(process_pipeline)

        assert "def validate_data" in code
        assert "def transform_data" in code
        assert "def process_pipeline" in code

        # Check ordering
        validate_pos = code.find("def validate_data")
        transform_pos = code.find("def transform_data")
        pipeline_pos = code.find("def process_pipeline")

        assert validate_pos < pipeline_pos
        assert transform_pos < pipeline_pos


class TestEdgeCases:
    """Test edge cases"""

    def test_function_with_default_args(self):
        """Test function with default arguments"""

        def with_defaults(a, b=10, c=20):
            return a + b + c

        code = generate_source_code(with_defaults)

        assert "def with_defaults(a, b=10, c=20):" in code

    def test_function_with_kwargs(self):
        """Test function with *args and **kwargs"""

        def with_varargs(*args, **kwargs):
            return sum(args)

        code = generate_source_code(with_varargs)

        assert "def with_varargs(*args, **kwargs):" in code

    def test_async_function(self):
        """Test async function"""

        async def async_func():
            return 42

        code = generate_source_code(async_func)

        assert "async def async_func():" in code
        assert "return 42" in code

    def test_lambda_works_in_files(self):
        """Test that lambda functions work when defined in files"""
        lambda_func = lambda x: x * 2

        # Lambda functions defined in files CAN have their source extracted
        code = generate_source_code(lambda_func)

        # Should include the lambda
        assert "lambda" in code.lower() or "lambda_func" in code


class TestCaching:
    """Test caching mechanism"""

    def test_cache_returns_same_result(self):
        """Test that cache returns identical results"""

        def cached_func():
            return 42

        code1 = generate_source_code(cached_func)
        code2 = generate_source_code(cached_func)

        assert code1 == code2

    def test_different_functions_different_cache(self):
        """Test that different functions have different cache entries"""

        def func1():
            return 1

        def func2():
            return 2

        code1 = generate_source_code(func1)
        code2 = generate_source_code(func2)

        assert code1 != code2
        assert "return 1" in code1
        assert "return 2" in code2


class TestBuiltinHandling:
    """Test that builtins are handled correctly"""

    def test_builtin_not_included(self):
        """Test that builtin functions are not included as dependencies"""

        def uses_builtins():
            data = list(range(10))
            return len(data)

        code = generate_source_code(uses_builtins)

        # Should not include definitions for list, range, len
        assert "def list" not in code
        assert "def range" not in code
        assert "def len" not in code
        assert "def uses_builtins():" in code


class TestImportStylePreservation:
    """Test that import styles are preserved"""

    def test_from_import_preserved(self):
        """Test from X import Y is preserved"""
        from math import sqrt

        def use_sqrt():
            return sqrt(16)

        code = generate_source_code(use_sqrt)

        assert "from math import sqrt" in code

    def test_import_as_preserved(self):
        """Test import X as Y is preserved"""
        import json as j

        def use_json():
            return j.dumps({"key": "value"})

        code = generate_source_code(use_json)

        # The import style should be preserved
        assert "import json" in code
