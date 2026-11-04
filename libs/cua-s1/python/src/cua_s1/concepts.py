"""Catalogue of form-field concepts with label synonyms and value generators.

Each concept describes one kind of information (a phone number, a policy
number, ...). A form shows it under one of `form_labels`; a source document
lists it under one of `doc_labels`. The synthetic generator samples both
sides independently so the model has to learn that "Tel" and "Phone number"
are the same thing while "Emergency contact phone" is not.
"""

from __future__ import annotations

import random
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any

PersonData = dict[str, str]
ValueGenerator = Callable[[random.Random, PersonData], str]

FIRST_NAMES = (
    "Maya",
    "Liam",
    "Aisha",
    "Noah",
    "Priya",
    "Ethan",
    "Sofia",
    "Mateo",
    "Hannah",
    "Kenji",
    "Olivia",
    "Diego",
    "Amara",
    "Lucas",
    "Zara",
    "Omar",
    "Isla",
    "Felix",
    "Nadia",
    "Tomas",
    "Grace",
    "Ravi",
    "Elena",
    "Jonas",
    "Leila",
    "Marcus",
    "Chloe",
    "Ibrahim",
    "Ruth",
    "Dmitri",
)
LAST_NAMES = (
    "Okafor",
    "Nguyen",
    "Schmidt",
    "Garcia",
    "Patel",
    "Kowalski",
    "Haddad",
    "Fischer",
    "Moreau",
    "Tanaka",
    "Silva",
    "Andersen",
    "Rossi",
    "Ivanova",
    "Mbeki",
    "O'Brien",
    "Larsson",
    "Delgado",
    "Yamamoto",
    "Novak",
    "Chen",
    "Becker",
    "Abernathy",
    "Quinn",
    "Farouk",
    "Lindqvist",
    "Ortega",
)
STREETS = (
    "Maple",
    "Oak",
    "Cedar",
    "Harbor",
    "Lakeview",
    "Sunset",
    "Ridge",
    "Park",
    "Willow",
    "Birch",
    "Station",
    "Mill",
    "Highland",
    "Elm",
    "Bridge",
)
STREET_TYPES = ("St", "Ave", "Rd", "Blvd", "Lane", "Drive", "Court", "Way")
CITIES = (
    ("Portland", "OR", "97205"),
    ("Austin", "TX", "78701"),
    ("Denver", "CO", "80202"),
    ("Boston", "MA", "02108"),
    ("Seattle", "WA", "98101"),
    ("Madison", "WI", "53703"),
    ("Raleigh", "NC", "27601"),
    ("Tucson", "AZ", "85701"),
    ("Omaha", "NE", "68102"),
    ("Savannah", "GA", "31401"),
    ("Boise", "ID", "83702"),
    ("Burlington", "VT", "05401"),
)
COMPANIES = (
    "Example Robotics",
    "Example Trading",
    "Test Manufacturing",
    "Example Health",
    "Test Logistics",
    "Example Industries",
    "Test Software",
    "Example Foods",
    "Test Freight",
    "Example Design",
    "Test Energy",
    "Example Services",
)
INSURERS = (
    "Example Health Plan",
    "Test Mutual",
    "Example Care",
    "Test Assurance",
    "Example Benefit Group",
    "Test Community Health",
)
JOB_TITLES = (
    "Software Engineer",
    "Nurse Practitioner",
    "Account Manager",
    "Data Analyst",
    "Robotics Technician",
    "Product Designer",
    "Warehouse Supervisor",
    "Dental Hygienist",
    "Civil Engineer",
    "Sales Associate",
    "Registered Nurse",
    "Project Manager",
)
CAR_MAKES = (
    ("Example Motors", "Alpha"),
    ("Test Automotive", "Beta"),
    ("Example Electric", "Gamma"),
    ("Test Vehicles", "Delta"),
)
COUNTRIES = ("United States", "Canada", "United Kingdom", "Germany", "Australia", "Japan", "Brazil")
BANKS = ("Example Bank", "Test Credit Union", "Example Savings", "Test Community Bank")
UNIVERSITIES = (
    "Example State University",
    "Test Technical Institute",
    "Example College",
    "Test Polytechnic",
    "Example Community College",
)
DEGREES = (
    "BSc Computer Science",
    "BA Economics",
    "MSc Nursing",
    "BEng Mechanical",
    "MBA",
    "Associate of Arts",
    "PhD Chemistry",
)
RELATIONS = (
    "Spouse",
    "Mother",
    "Father",
    "Sister",
    "Brother",
    "Partner",
    "Friend",
    "Daughter",
    "Son",
)
REASONS = (
    "Annual check-up",
    "Persistent cough for two weeks",
    "Follow-up after surgery",
    "Knee pain when climbing stairs",
    "Flu vaccination",
    "Migraine headaches",
    "Back pain",
    "Skin rash on left arm",
    "Blood pressure review",
    "Allergy testing",
)
BLOOD_TYPES = ("A+", "A-", "B+", "B-", "O+", "O-", "AB+", "AB-")


def _digits(rng: random.Random, n: int) -> str:
    return "".join(str(rng.randint(0, 9)) for _ in range(n))


def _alnum(rng: random.Random, n: int) -> str:
    return "".join(rng.choice("ABCDEFGHJKLMNPQRSTUVWXYZ0123456789") for _ in range(n))


def gen_phone(rng: random.Random) -> str:
    suffix = rng.randint(100, 199)
    return rng.choice(
        (
            f"(202) 555-{suffix:04d}",
            f"202-555-{suffix:04d}",
            f"202.555.{suffix:04d}",
            f"+1 202 555 {suffix:04d}",
        )
    )


def gen_date(rng: random.Random, start: int = 1940, end: int = 2008) -> str:
    y, m, d = rng.randint(start, end), rng.randint(1, 12), rng.randint(1, 28)
    return rng.choice(
        (
            f"{m:02d}/{d:02d}/{y}",
            f"{y}-{m:02d}-{d:02d}",
            f"{d} {['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][m - 1]} {y}",
        )
    )


def gen_money(rng: random.Random) -> str:
    v = rng.randint(30, 220) * 1000
    return rng.choice((f"${v:,}", f"{v} USD", f"${v // 1000}k"))


@dataclass
class Concept:
    key: str
    form_labels: tuple[str, ...]
    doc_labels: tuple[str, ...]
    value: ValueGenerator
    kind: str = "text"  # text | textarea | select
    placeholder: tuple[str, ...] = ()
    group: str = "general"  # concepts in the same group are plausible confusers
    options: tuple[str, ...] = ()  # for select kind


def C(
    key: str,
    form_labels: Sequence[str],
    doc_labels: Sequence[str],
    value: ValueGenerator,
    **kwargs: Any,
) -> Concept:
    """Build a concept while normalizing label collections to tuples."""
    return Concept(key, tuple(form_labels), tuple(doc_labels), value, **kwargs)


# Person-level values are generated once per "person" so derived fields agree.
def person(rng: random.Random) -> PersonData:
    """Generate one internally consistent synthetic person's field values."""
    first, last = rng.choice(FIRST_NAMES), rng.choice(LAST_NAMES)
    city, state, zipc = rng.choice(CITIES)
    ec_first, ec_last = rng.choice(FIRST_NAMES), rng.choice(LAST_NAMES)
    make, model = rng.choice(CAR_MAKES)
    domain = rng.choice(("example.invalid", "mail.example.invalid", "test.example.invalid"))
    return {
        "first": first,
        "last": last,
        "full": f"{first} {last}",
        "email": rng.choice(
            (
                f"{first.lower()}.{last.lower().replace(chr(39), '')}@{domain}",
                f"{first[0].lower()}{last.lower().replace(chr(39), '')}{rng.randint(1, 99)}@{domain}",
            )
        ),
        "phone": gen_phone(rng),
        "work_phone": gen_phone(rng),
        "dob": gen_date(rng),
        "street": f"{rng.randint(10, 9999)} {rng.choice(STREETS)} {rng.choice(STREET_TYPES)}",
        "unit": rng.choice(("Apt 4B", "Unit 12", "Suite 300", "#7", "Apt 2")),
        "city": city,
        "state": state,
        "zip": zipc,
        "country": rng.choice(COUNTRIES),
        "insurer": rng.choice(INSURERS),
        "policy": f"TEST-POL-{_digits(rng, 8)}",
        "group_no": f"TEST-GRP-{_digits(rng, 6)}",
        "member_id": f"TEST-MBR-{_alnum(rng, 9)}",
        "ec_name": f"{ec_first} {ec_last}",
        "ec_phone": gen_phone(rng),
        "ec_relation": rng.choice(RELATIONS),
        "employer": rng.choice(COMPANIES),
        "job_title": rng.choice(JOB_TITLES),
        "years": str(rng.randint(1, 25)),
        "salary": gen_money(rng),
        "start": gen_date(rng, 2026, 2027),
        "linkedin": f"https://profiles.example.invalid/{first.lower()}-{last.lower().replace(chr(39), '')}",
        "website": f"https://{last.lower().replace(chr(39), '')}.example.invalid",
        "ssn": f"900-{_digits(rng, 2)}-{_digits(rng, 4)}",
        "license": f"TEST-DL-{_alnum(rng, 8)}",
        "passport": f"TEST-PPT-{_alnum(rng, 8)}",
        "car_make": make,
        "car_model": model,
        "car_year": str(rng.randint(2008, 2026)),
        "plate": f"TEST-{_alnum(rng, 3)}",
        "vin": f"TESTVIN-IQ-{_alnum(rng, 7)}",
        "bank": rng.choice(BANKS),
        "account": f"TEST-ACCT-{_digits(rng, 10)}",
        "routing": f"TEST-ROUTE-{_digits(rng, 9)}",
        "university": rng.choice(UNIVERSITIES),
        "degree": rng.choice(DEGREES),
        "grad_year": str(rng.randint(1995, 2025)),
        "reason": rng.choice(REASONS),
        "blood": rng.choice(BLOOD_TYPES),
        "allergies": rng.choice(("Penicillin", "None", "Peanuts", "Latex", "Shellfish")),
        "medications": rng.choice(
            ("None", "Metformin 500mg", "Lisinopril 10mg", "Albuterol inhaler")
        ),
        "doctor": f"Dr. {rng.choice(LAST_NAMES)}",
        "sex": rng.choice(("Female", "Male")),
        "marital": rng.choice(("Single", "Married", "Divorced", "Widowed")),
        "nationality": rng.choice(
            ("American", "Canadian", "British", "German", "Indian", "Japanese")
        ),
        "company_reg": f"TEST-REG-{_digits(rng, 7)}",
        "tax_id": f"00-{_digits(rng, 7)}",
        "claim_no": f"TEST-CLM-{_digits(rng, 6)}",
        "incident_date": gen_date(rng, 2025, 2026),
        "incident_desc": rng.choice(
            (
                "Rear-ended at a stop light",
                "Water damage from burst pipe",
                "Lost luggage on connecting flight",
                "Slipped on icy pavement",
            )
        ),
        "cover": rng.choice(
            (
                "I am excited to apply for this role and bring my experience to your team.",
                "With five years in the field I believe I would be a strong fit.",
                "Please consider my application for the open position.",
            )
        ),
    }


CONCEPTS: list[Concept] = [
    C(
        "first_name",
        ("First name", "Given name", "First Name", "Forename", "First"),
        ("First name", "Given name", "First", "Forename"),
        lambda r, p: p["first"],
        placeholder=("Given name", "First"),
        group="name",
    ),
    C(
        "last_name",
        ("Last name", "Surname", "Family name", "Last Name", "Last"),
        ("Last name", "Surname", "Family name", "Last"),
        lambda r, p: p["last"],
        placeholder=("Family name",),
        group="name",
    ),
    C(
        "full_name",
        (
            "Full name",
            "Name",
            "Your name",
            "Applicant name",
            "Patient name",
            "Legal name",
            "Name (as on ID)",
        ),
        ("Name", "Full name", "Patient", "Applicant", "Legal name", "Insured"),
        lambda r, p: p["full"],
        group="name",
    ),
    C(
        "email",
        ("Email address", "Email", "E-mail", "Contact email", "Your email", "Email Address"),
        ("Email", "E-mail", "Email address", "Contact"),
        lambda r, p: p["email"],
        placeholder=("you@example.com", "name@domain.com"),
        group="contact",
    ),
    C(
        "phone",
        (
            "Phone number",
            "Phone",
            "Telephone",
            "Mobile phone",
            "Contact phone",
            "Primary phone",
            "Cell phone",
            "Mobile number",
            "Daytime phone",
        ),
        ("Phone", "Tel", "Telephone", "Mobile", "Cell", "Contact number", "Primary phone"),
        lambda r, p: p["phone"],
        placeholder=("(555) 555-5555",),
        group="contact",
    ),
    C(
        "work_phone",
        ("Work phone", "Office phone", "Business phone", "Work telephone"),
        ("Work phone", "Office", "Business tel", "Work"),
        lambda r, p: p["work_phone"],
        group="contact",
    ),
    C(
        "dob",
        ("Date of birth", "Birth date", "DOB", "Birthday", "Date of Birth"),
        ("DOB", "Date of birth", "Born", "Birth date", "Birthday"),
        lambda r, p: p["dob"],
        placeholder=("MM/DD/YYYY", "YYYY-MM-DD"),
        group="dates",
    ),
    C(
        "street",
        (
            "Street address",
            "Address",
            "Address line 1",
            "Street",
            "Home address",
            "Mailing address",
            "Residential address",
        ),
        ("Address", "Street", "Home address", "Residence", "Address line 1"),
        lambda r, p: p["street"],
        group="address",
    ),
    C(
        "unit",
        ("Address line 2", "Apt / Suite", "Apartment or unit", "Unit number"),
        ("Apt", "Unit", "Suite", "Address line 2"),
        lambda r, p: p["unit"],
        group="address",
    ),
    C(
        "city",
        ("City", "Town", "City / Town", "Town or city"),
        ("City", "Town"),
        lambda r, p: p["city"],
        group="address",
    ),
    C(
        "state",
        ("State", "State / Province", "Province", "Region"),
        ("State", "Province", "Region", "ST"),
        lambda r, p: p["state"],
        group="address",
    ),
    C(
        "zip",
        ("ZIP code", "Postal code", "Zip", "ZIP", "Postcode", "Zip / Postal code"),
        ("ZIP", "Zip code", "Postal code", "Postcode"),
        lambda r, p: p["zip"],
        placeholder=("ZIP", "12345"),
        group="address",
    ),
    C(
        "country",
        ("Country", "Country of residence", "Nation"),
        ("Country", "Country of residence"),
        lambda r, p: p["country"],
        group="address",
    ),
    C(
        "insurer",
        (
            "Insurance provider",
            "Insurance company",
            "Insurer",
            "Health plan",
            "Carrier",
            "Insurance carrier",
        ),
        ("Insurer", "Insurance", "Carrier", "Health plan", "Insurance company", "Plan"),
        lambda r, p: p["insurer"],
        group="insurance",
    ),
    C(
        "policy",
        ("Policy number", "Policy #", "Policy no.", "Insurance policy number", "Policy ID"),
        ("Policy", "Policy #", "Policy no", "Policy number", "Policy ID"),
        lambda r, p: p["policy"],
        placeholder=("Policy #",),
        group="insurance",
    ),
    C(
        "group_no",
        ("Group number", "Group #", "Plan group number"),
        ("Group", "Group #", "Group no", "Group number"),
        lambda r, p: p["group_no"],
        group="insurance",
    ),
    C(
        "member_id",
        ("Member ID", "Subscriber ID", "Member number", "Insurance ID"),
        ("Member ID", "Subscriber", "Member #", "ID number"),
        lambda r, p: p["member_id"],
        group="insurance",
    ),
    C(
        "ec_name",
        (
            "Emergency contact name",
            "Emergency contact",
            "In case of emergency contact",
            "Next of kin",
            "Emergency contact (name)",
        ),
        (
            "Emergency contact",
            "ICE",
            "Next of kin",
            "In case of emergency",
            "Emergency contact name",
        ),
        lambda r, p: p["ec_name"],
        group="emergency",
    ),
    C(
        "ec_phone",
        (
            "Emergency contact phone",
            "Emergency phone",
            "Emergency contact number",
            "Next of kin phone",
            "Emergency contact telephone",
        ),
        (
            "Emergency phone",
            "ICE phone",
            "Emergency contact phone",
            "Next of kin tel",
            "Emergency number",
        ),
        lambda r, p: p["ec_phone"],
        group="emergency",
    ),
    C(
        "ec_relation",
        (
            "Relationship to patient",
            "Emergency contact relationship",
            "Relationship",
            "Relation to applicant",
        ),
        ("Relationship", "Relation", "Emergency contact relation"),
        lambda r, p: p["ec_relation"],
        group="emergency",
    ),
    C(
        "employer",
        (
            "Current employer",
            "Employer",
            "Company",
            "Company name",
            "Organization",
            "Current company",
        ),
        ("Employer", "Company", "Organization", "Works at", "Current employer"),
        lambda r, p: p["employer"],
        group="employment",
    ),
    C(
        "job_title",
        ("Current job title", "Job title", "Position", "Occupation", "Title", "Role"),
        ("Title", "Job title", "Occupation", "Position", "Role", "Profession"),
        lambda r, p: p["job_title"],
        group="employment",
    ),
    C(
        "years",
        ("Years of experience", "Experience (years)", "Total years experience", "Years in role"),
        ("Experience", "Years of experience", "Years", "Yrs experience"),
        lambda r, p: p["years"],
        group="employment",
    ),
    C(
        "salary",
        (
            "Desired salary",
            "Expected salary",
            "Salary expectation",
            "Current salary",
            "Annual salary",
        ),
        ("Salary", "Expected salary", "Desired compensation", "Compensation", "Annual pay"),
        lambda r, p: p["salary"],
        group="employment",
    ),
    C(
        "start_date",
        (
            "Earliest start date",
            "Available from",
            "Start date",
            "Availability date",
            "Date available",
        ),
        ("Start date", "Available", "Availability", "Can start", "Earliest start"),
        lambda r, p: p["start"],
        group="dates",
    ),
    C(
        "linkedin",
        ("LinkedIn profile URL", "LinkedIn", "LinkedIn profile", "Professional profile URL"),
        ("LinkedIn", "Profile", "LinkedIn URL"),
        lambda r, p: p["linkedin"],
        group="web",
    ),
    C(
        "website",
        ("Website", "Personal website", "Portfolio URL", "Homepage", "Web site"),
        ("Website", "Web", "Portfolio", "URL", "Homepage"),
        lambda r, p: p["website"],
        group="web",
    ),
    C(
        "ssn",
        ("Social Security Number", "SSN", "Social security no.", "National ID number"),
        ("SSN", "Social Security", "Social security number", "National ID"),
        lambda r, p: p["ssn"],
        group="ids",
    ),
    C(
        "license",
        ("Driver's license number", "Driver license #", "License number", "DL number"),
        ("License", "DL", "Driver's license", "License no", "Driver license"),
        lambda r, p: p["license"],
        group="ids",
    ),
    C(
        "passport",
        ("Passport number", "Passport #", "Passport no."),
        ("Passport", "Passport #", "Passport number"),
        lambda r, p: p["passport"],
        group="ids",
    ),
    C(
        "car_make",
        ("Vehicle make", "Make", "Car make", "Manufacturer"),
        ("Make", "Vehicle make", "Manufacturer"),
        lambda r, p: p["car_make"],
        group="vehicle",
    ),
    C(
        "car_model",
        ("Vehicle model", "Model", "Car model"),
        ("Model", "Vehicle model"),
        lambda r, p: p["car_model"],
        group="vehicle",
    ),
    C(
        "car_year",
        ("Vehicle year", "Model year", "Year"),
        ("Year", "Model year", "Vehicle year"),
        lambda r, p: p["car_year"],
        group="vehicle",
    ),
    C(
        "plate",
        ("License plate", "Plate number", "Registration plate", "Plate #"),
        ("Plate", "License plate", "Registration", "Reg plate"),
        lambda r, p: p["plate"],
        group="vehicle",
    ),
    C(
        "vin",
        ("VIN", "Vehicle identification number", "VIN number"),
        ("VIN", "Vehicle ID", "Chassis number"),
        lambda r, p: p["vin"],
        group="vehicle",
    ),
    C(
        "bank",
        ("Bank name", "Bank", "Financial institution", "Name of bank"),
        ("Bank", "Institution", "Bank name"),
        lambda r, p: p["bank"],
        group="banking",
    ),
    C(
        "account",
        ("Account number", "Bank account number", "Account #", "Acct number"),
        ("Account", "Account #", "Acct", "Account number"),
        lambda r, p: p["account"],
        group="banking",
    ),
    C(
        "routing",
        ("Routing number", "ABA routing number", "Routing #", "Sort code"),
        ("Routing", "ABA", "Routing number", "Sort code"),
        lambda r, p: p["routing"],
        group="banking",
    ),
    C(
        "university",
        ("University", "School", "Institution attended", "College", "School name"),
        ("University", "School", "College", "Education", "Alma mater"),
        lambda r, p: p["university"],
        group="education",
    ),
    C(
        "degree",
        ("Degree", "Qualification", "Highest degree", "Degree earned"),
        ("Degree", "Qualification", "Studied"),
        lambda r, p: p["degree"],
        group="education",
    ),
    C(
        "grad_year",
        ("Graduation year", "Year graduated", "Year of graduation"),
        ("Graduated", "Class of", "Graduation", "Grad year"),
        lambda r, p: p["grad_year"],
        group="education",
    ),
    C(
        "reason",
        (
            "Reason for visit",
            "Chief complaint",
            "Purpose of visit",
            "Symptoms",
            "Describe your concern",
        ),
        ("Reason", "Complaint", "Reason for visit", "Presenting problem", "Symptoms"),
        lambda r, p: p["reason"],
        kind="textarea",
        group="medical",
    ),
    C(
        "blood",
        ("Blood type", "Blood group"),
        ("Blood type", "Blood group", "Blood"),
        lambda r, p: p["blood"],
        group="medical",
    ),
    C(
        "allergies",
        ("Known allergies", "Allergies", "Drug allergies", "List any allergies"),
        ("Allergies", "Allergic to", "Known allergies"),
        lambda r, p: p["allergies"],
        group="medical",
    ),
    C(
        "medications",
        ("Current medications", "Medications", "Prescriptions", "Medicines taken"),
        ("Medications", "Meds", "Prescriptions", "Taking"),
        lambda r, p: p["medications"],
        group="medical",
    ),
    C(
        "doctor",
        ("Primary care physician", "Referring doctor", "Doctor's name", "Family doctor", "GP name"),
        ("Physician", "Doctor", "Referred by", "PCP", "GP"),
        lambda r, p: p["doctor"],
        group="medical",
    ),
    C(
        "sex",
        ("Sex", "Gender", "Sex assigned at birth"),
        ("Sex", "Gender"),
        lambda r, p: p["sex"],
        group="demographic",
    ),
    C(
        "marital",
        ("Marital status", "Relationship status"),
        ("Marital status", "Married?", "Status"),
        lambda r, p: p["marital"],
        group="demographic",
    ),
    C(
        "nationality",
        ("Nationality", "Citizenship"),
        ("Nationality", "Citizen of", "Citizenship"),
        lambda r, p: p["nationality"],
        group="demographic",
    ),
    C(
        "tax_id",
        ("Tax ID", "Taxpayer identification number", "EIN", "Tax number", "TIN"),
        ("Tax ID", "EIN", "TIN", "Taxpayer ID"),
        lambda r, p: p["tax_id"],
        group="business",
    ),
    C(
        "company_reg",
        ("Company registration number", "Business registration #", "Registration number"),
        ("Registration", "Reg no", "Company reg", "Business registration"),
        lambda r, p: p["company_reg"],
        group="business",
    ),
    C(
        "claim_no",
        ("Claim number", "Claim #", "Claim reference", "Case number"),
        ("Claim", "Claim #", "Reference", "Case", "Claim number"),
        lambda r, p: p["claim_no"],
        group="claim",
    ),
    C(
        "incident_date",
        ("Date of incident", "Incident date", "Date of loss", "Accident date", "Date of accident"),
        ("Incident", "Date of loss", "Occurred", "Accident date", "Date of incident"),
        lambda r, p: p["incident_date"],
        group="dates",
    ),
    C(
        "incident_desc",
        ("Description of incident", "What happened?", "Describe the incident", "Details of loss"),
        ("Description", "Incident", "What happened", "Details", "Summary"),
        lambda r, p: p["incident_desc"],
        kind="textarea",
        group="claim",
    ),
    C(
        "cover",
        (
            "Cover letter",
            "Why do you want this job?",
            "Message to hiring manager",
            "Additional comments",
        ),
        ("Cover letter", "Statement", "Note", "Message"),
        lambda r, p: p["cover"],
        kind="textarea",
        group="employment",
    ),
]

CONCEPT_BY_KEY = {c.key: c for c in CONCEPTS}

# Fields that appear in forms but never have a document entity: model should skip.
OPTIONAL_FIELDS = (
    "Promo code (optional)",
    "Referral code (optional)",
    "Coupon code",
    "Discount code",
    "How did you hear about us?",
    "Additional notes (optional)",
    "Comments",
    "Middle name (optional)",
    "Fax number",
    "Second email (optional)",
    "Preferred nickname",
    "Employee ID (staff only)",
    "Internal reference",
    "Agent code",
    "Confirmation code",
    "CAPTCHA answer",
    "Voucher",
)

# Distractor document entities that match no field on the form (or a field on another form).
DISTRACTOR_ENTITIES = (
    ("Document ID", lambda r: f"DOC-{_digits(r, 6)}"),
    ("Printed", lambda r: gen_date(r, 2026, 2026)),
    ("Page", lambda r: f"1 of {r.randint(1, 4)}"),
    ("Prepared by", lambda r: f"{r.choice(FIRST_NAMES)} {r.choice(LAST_NAMES)}"),
    ("Fax", lambda r: gen_phone(r)),
    ("Height", lambda r: f"{r.randint(150, 195)} cm"),
    ("Weight", lambda r: f"{r.randint(48, 120)} kg"),
    ("Case worker", lambda r: f"{r.choice(FIRST_NAMES)} {r.choice(LAST_NAMES)}"),
    ("Office", lambda r: r.choice(("Downtown", "Eastside", "North campus"))),
    ("Confidential", lambda r: "Yes"),
    ("Reference", lambda r: _alnum(r, 8)),
    ("Signature", lambda r: "on file"),
    ("Language", lambda r: r.choice(("English", "Spanish", "French"))),
)

SUBMIT_LABELS = (
    "Submit",
    "Submit registration",
    "Submit application",
    "Submit form",
    "Send",
    "Continue",
    "Register",
    "Apply now",
    "Save and continue",
    "Complete registration",
    "Next",
    "Finish",
    "Submit claim",
    "Confirm and submit",
    "Sign up",
    "Book appointment",
    "Create account",
    "Done",
)
NON_SUBMIT_BUTTONS = (
    "Clear form",
    "Cancel",
    "Reset",
    "Clear",
    "Discard",
    "Back",
    "Previous",
    "Go back",
    "Delete draft",
    "Log out",
    "Help",
    "Print blank form",
    "Upload file",
    "Add another",
    "Remove",
    "Start over",
    "Report a problem",
    "Close",
    "Exit",
)
REQUIRED_CHECKBOXES = (
    "I consent to treatment and agree to the privacy policy",
    "I agree to the terms and conditions",
    "I certify the information above is accurate",
    "I am legally authorized to work in this country",
    "I have read and accept the privacy notice",
    "I confirm the details are correct",
    "I accept the terms of service",
    "Consent to electronic communication (required)",
    "I acknowledge the cancellation policy",
    "I agree to the HIPAA notice",
    "I authorize verification of the information provided",
    "I understand this is a binding application",
)
OPTIONAL_CHECKBOXES = (
    "Subscribe to our newsletter",
    "Send me promotional offers",
    "Text me appointment reminders",
    "Share my data with partners",
    "Remember me on this device",
    "Join the loyalty program",
    "I would like to receive marketing email",
    "Add me to the mailing list",
    "Sign up for SMS alerts",
)
CHROME_ELEMENTS = (
    ("Button", "Back"),
    ("Button", "Forward"),
    ("Button", "Reload"),
    ("Edit", "Address bar"),
    ("Button", "Go"),
    ("Pane", "Page content"),
    ("ScrollBar", "Vertical"),
    ("Button", "Line up"),
    ("Button", "Line down"),
    ("Button", "Page down"),
    ("Button", "Page up"),
    ("TitleBar", ""),
    ("Button", "Minimise"),
    ("Button", "Maximise"),
    ("Button", "Close"),
    ("MenuItem", "System"),
    ("Button", "Minimize"),
    ("Button", "Maximize"),
    ("Button", "Bookmark this page"),
    ("Button", "New tab"),
    ("Button", "Settings and more"),
    ("Edit", "Search"),
    ("Button", "Zoom"),
)
FORM_TITLES = (
    "Example Clinic - New Patient Registration",
    "Example Robotics - Job Application",
    "Test Insurance - Auto Claim Form",
    "Example Library - Membership Sign-up",
    "Test Bank - Direct Deposit Enrollment",
    "Example Dental - Patient Intake",
    "Test University - Graduate Application",
    "Example Utilities - New Service Request",
    "Test Freight - Vendor Onboarding",
    "Example Vet - Pet Owner Registration",
    "Test Rentals - Tenant Application",
    "Example Travel - Visa Assistance Form",
    "Test Camp - Parent Consent Form",
    "Example Telecom - Account Transfer",
    "Test Pharmacy - Prescription Transfer",
    "Example Gym - Member Enrollment",
)
