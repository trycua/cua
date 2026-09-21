"""Entity/value generation for the synthetic task generator.

A `Concept` is one kind of real-world value (an email, a policy number, a
route/date pair for search_filter, a username/password pair for login_auth,
...). Each concept carries label synonyms for both the "field" side (what a
form/UI element calls it) and the "source" side (what a document/entity list
calls it), plus a value generator. Kept intentionally small -- wide enough to
cover 2-3 example apps per family without becoming a large maintenance
surface; add concepts as new example apps need them, following this same
pattern.
"""
from __future__ import annotations

import random
from dataclasses import dataclass
from typing import Callable

FIRST_NAMES = ("Maya", "Liam", "Aisha", "Noah", "Priya", "Ethan", "Sofia", "Mateo", "Hannah", "Kenji",
               "Olivia", "Diego", "Amara", "Lucas", "Zara", "Omar")
LAST_NAMES = ("Okafor", "Nguyen", "Schmidt", "Garcia", "Patel", "Kowalski", "Haddad", "Fischer",
              "Moreau", "Tanaka", "Silva", "Andersen", "Rossi", "Ivanova")
STREETS = ("Maple", "Oak", "Cedar", "Harbor", "Lakeview", "Sunset", "Ridge", "Park", "Willow", "Birch")
STREET_TYPES = ("St", "Ave", "Rd", "Blvd", "Lane", "Drive")
CITIES = (("Portland", "OR", "97205"), ("Austin", "TX", "78701"), ("Denver", "CO", "80202"),
          ("Boston", "MA", "02108"), ("Seattle", "WA", "98101"), ("Madison", "WI", "53703"))
COMPANIES = ("Acme Robotics", "Northwind Traders", "Globex Corp", "Initech", "Umbrella Health",
             "Stark Logistics", "Vandelay Industries", "Hooli")
AIRPORTS = (("JFK", "New York"), ("LAX", "Los Angeles"), ("ORD", "Chicago"), ("SEA", "Seattle"),
            ("DEN", "Denver"), ("ATL", "Atlanta"), ("SFO", "San Francisco"), ("BOS", "Boston"))
PRODUCT_CATS = ("Electronics", "Home & Kitchen", "Books", "Clothing", "Sporting Goods", "Toys")


def _digits(rng: random.Random, n: int) -> str:
    return "".join(str(rng.randint(0, 9)) for _ in range(n))


def _alnum(rng: random.Random, n: int) -> str:
    return "".join(rng.choice("ABCDEFGHJKLMNPQRSTUVWXYZ0123456789") for _ in range(n))


def gen_phone(rng: random.Random) -> str:
    a, b, c = rng.randint(200, 989), rng.randint(200, 999), rng.randint(1000, 9999)
    return rng.choice((f"({a}) {b}-{c}", f"{a}-{b}-{c}", f"+1 {a} {b} {c}"))


def gen_date(rng: random.Random, start: int = 2026, end: int = 2027) -> str:
    y, m, d = rng.randint(start, end), rng.randint(1, 12), rng.randint(1, 28)
    return rng.choice((f"{m:02d}/{d:02d}/{y}", f"{y}-{m:02d}-{d:02d}"))


def gen_money(rng: random.Random) -> str:
    return f"${rng.randint(30, 900)}"


@dataclass
class Concept:
    key: str
    field_labels: tuple[str, ...]     # how a UI element might label this value
    source_labels: tuple[str, ...]    # how a source document/entity list might label it
    value: Callable[[random.Random, dict], str]
    group: str = "general"            # concepts in the same group are plausible confusers


def C(key, field_labels, source_labels, value, **kw) -> Concept:
    return Concept(key, tuple(field_labels), tuple(source_labels), value, **kw)


def person(rng: random.Random) -> dict:
    """Person-level values generated once so derived fields (email from name, etc.) agree."""
    first, last = rng.choice(FIRST_NAMES), rng.choice(LAST_NAMES)
    city, state, zipc = rng.choice(CITIES)
    domain = rng.choice(("gmail.com", "outlook.com", "proton.me", "fastmail.com"))
    dep, arr = rng.sample(AIRPORTS, 2)
    return {
        "first": first, "last": last, "full": f"{first} {last}",
        "email": f"{first.lower()}.{last.lower()}@{domain}",
        "phone": gen_phone(rng), "dob": gen_date(rng, 1960, 2008),
        "street": f"{rng.randint(10, 9999)} {rng.choice(STREETS)} {rng.choice(STREET_TYPES)}",
        "city": city, "state": state, "zip": zipc,
        "employer": rng.choice(COMPANIES), "policy": f"POL-{_digits(rng, 8)}",
        "username": f"{first.lower()}{rng.randint(1, 999)}",
        "password": _alnum(rng, 10),
        "dep_code": dep[0], "dep_city": dep[1], "arr_code": arr[0], "arr_city": arr[1],
        "depart_date": gen_date(rng), "return_date": gen_date(rng),
        "passengers": str(rng.randint(1, 4)), "cabin": rng.choice(("Economy", "Premium", "Business")),
        "search_query": rng.choice(("wireless headphones", "running shoes", "coffee maker", "desk lamp")),
        "category": rng.choice(PRODUCT_CATS), "max_price": gen_money(rng),
    }


CONCEPTS: list[Concept] = [
    C("first_name", ("First name", "Given name"), ("First name", "Given name"), lambda r, p: p["first"], group="name"),
    C("last_name", ("Last name", "Surname"), ("Last name", "Surname"), lambda r, p: p["last"], group="name"),
    C("full_name", ("Full name", "Name", "Your name"), ("Name", "Full name"), lambda r, p: p["full"], group="name"),
    C("email", ("Email address", "Email", "E-mail"), ("Email", "E-mail"), lambda r, p: p["email"], group="contact"),
    C("phone", ("Phone number", "Phone", "Mobile"), ("Phone", "Tel", "Mobile"), lambda r, p: p["phone"], group="contact"),
    C("dob", ("Date of birth", "Birthday"), ("DOB", "Date of birth"), lambda r, p: p["dob"], group="dates"),
    C("street", ("Street address", "Address"), ("Address", "Street"), lambda r, p: p["street"], group="address"),
    C("city", ("City",), ("City",), lambda r, p: p["city"], group="address"),
    C("state", ("State",), ("State",), lambda r, p: p["state"], group="address"),
    C("zip", ("ZIP code", "Postal code"), ("ZIP", "Postal code"), lambda r, p: p["zip"], group="address"),
    C("employer", ("Employer", "Company"), ("Employer", "Company"), lambda r, p: p["employer"], group="employment"),
    C("policy", ("Policy number", "Policy #"), ("Policy", "Policy number"), lambda r, p: p["policy"], group="insurance"),
    C("username", ("Username", "User ID", "Login"), ("Username", "Account"), lambda r, p: p["username"], group="auth"),
    C("password", ("Password",), ("Password",), lambda r, p: p["password"], group="auth"),
    C("dep_city", ("From", "Departure city", "Origin"), ("Origin", "From"), lambda r, p: p["dep_city"], group="travel"),
    C("arr_city", ("To", "Destination city", "Destination"), ("Destination", "To"), lambda r, p: p["arr_city"], group="travel"),
    C("depart_date", ("Departure date", "Depart"), ("Depart", "Departure"), lambda r, p: p["depart_date"], group="dates"),
    C("return_date", ("Return date", "Return"), ("Return",), lambda r, p: p["return_date"], group="dates"),
    C("passengers", ("Passengers", "Travelers"), ("Passengers",), lambda r, p: p["passengers"], group="travel"),
    C("cabin", ("Cabin class", "Class"), ("Cabin",), lambda r, p: p["cabin"], group="travel"),
    C("search_query", ("Search", "Search term", "Keyword"), ("Query",), lambda r, p: p["search_query"], group="search"),
    C("category", ("Category", "Department"), ("Category",), lambda r, p: p["category"], group="search"),
    C("max_price", ("Max price", "Price under"), ("Budget",), lambda r, p: p["max_price"], group="search"),
]
CONCEPT_BY_KEY = {c.key: c for c in CONCEPTS}

SUBMIT_LABELS = ("Submit", "Continue", "Next", "Save and continue", "Sign up", "Create account", "Search",
                 "Apply filters", "Log in", "Register")
NON_SUBMIT_BUTTONS = ("Cancel", "Reset", "Clear", "Back", "Help")
REQUIRED_CHECKBOXES = ("I agree to the terms and conditions", "I consent to the privacy policy",
                       "I certify the information above is accurate", "I accept the terms of service")
OPTIONAL_CHECKBOXES = ("Subscribe to our newsletter", "Remember me on this device", "Send me promotional offers")
