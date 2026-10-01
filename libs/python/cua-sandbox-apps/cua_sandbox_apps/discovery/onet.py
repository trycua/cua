"""Load O*NET occupation data and BLS employment/wage statistics.

This is static data processing — downloads CSV files from BLS/O*NET
and computes wage bills per occupation. No LLM calls needed.
"""

from __future__ import annotations

import csv
import json
import logging
from pathlib import Path
from typing import Optional

import httpx

logger = logging.getLogger(__name__)

# BLS OEWS data URL (May 2024 national estimates, all occupations)
BLS_OEWS_URL = "https://www.bls.gov/oes/special-requests/oesm24nat.zip"
# O*NET occupation data (SOC crosswalk)
ONET_SOC_URL = "https://www.onetcenter.org/dl_files/database/db_29_1_text/Occupation%20Data.txt"
# O*NET computer use importance/level
ONET_WORK_ACTIVITIES_URL = (
    "https://www.onetcenter.org/dl_files/database/db_29_1_text/Work%20Activities.txt"
)


async def download_if_missing(url: str, dest: Path, client: httpx.AsyncClient) -> Path:
    """Download a file if it doesn't exist locally."""
    if dest.exists():
        logger.info("Using cached %s", dest.name)
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    logger.info("Downloading %s → %s", url, dest)
    resp = await client.get(url, follow_redirects=True)
    resp.raise_for_status()
    dest.write_bytes(resp.content)
    return dest


def parse_oews_national(csv_path: Path) -> list[dict]:
    """Parse BLS OEWS national CSV into occupation records.

    Returns list of dicts with: soc_code, occupation_title, employment, mean_wage.
    """
    records = []
    with open(csv_path, newline="", encoding="utf-8-sig") as f:
        reader = csv.DictReader(f)
        for row in reader:
            # BLS uses "**" or "#" for missing data
            try:
                employment = int(row.get("TOT_EMP", "0").replace(",", ""))
            except ValueError:
                employment = 0
            try:
                mean_wage = float(row.get("A_MEAN", "0").replace(",", ""))
            except ValueError:
                mean_wage = 0.0
            if employment > 0 and mean_wage > 0:
                records.append(
                    {
                        "soc_code": row.get("OCC_CODE", ""),
                        "occupation_title": row.get("OCC_TITLE", ""),
                        "employment": employment,
                        "mean_wage": mean_wage,
                        "wage_bill": employment * mean_wage,
                    }
                )
    return records


def compute_gdp_per_occupation(
    occupations: list[dict],
    compensation_to_wage_ratio: float = 1.4,
    gdp_to_compensation_ratio: float = 1.8,
) -> list[dict]:
    """Scale wage bills to GDP estimates per occupation.

    Uses BEA national accounts ratios (approximate 2024 values):
    - Total Compensation / Total Wages ≈ 1.4
    - National GDP / National Compensation ≈ 1.8
    """
    for occ in occupations:
        occ["gdp_labor"] = occ["wage_bill"] * compensation_to_wage_ratio
        occ["gdp_total"] = occ["gdp_labor"] * gdp_to_compensation_ratio
    return occupations


def write_jsonl(records: list[dict], path: Path) -> None:
    """Append records to a JSONL file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8") as f:
        for rec in records:
            f.write(json.dumps(rec, default=str) + "\n")


def read_jsonl(path: Path) -> list[dict]:
    """Read all records from a JSONL file."""
    if not path.exists():
        return []
    records = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                records.append(json.loads(line))
    return records


# Fallback: generate synthetic occupation data from common SOC groups
# when BLS data isn't available locally
SOC_MAJOR_GROUPS = {
    "11-0000": "Management",
    "13-0000": "Business and Financial Operations",
    "15-0000": "Computer and Mathematical",
    "17-0000": "Architecture and Engineering",
    "19-0000": "Life, Physical, and Social Science",
    "21-0000": "Community and Social Service",
    "23-0000": "Legal",
    "25-0000": "Educational Instruction and Library",
    "27-0000": "Arts, Design, Entertainment, Sports, and Media",
    "29-0000": "Healthcare Practitioners and Technical",
    "31-0000": "Healthcare Support",
    "33-0000": "Protective Service",
    "35-0000": "Food Preparation and Serving Related",
    "37-0000": "Building and Grounds Cleaning and Maintenance",
    "39-0000": "Personal Care and Service",
    "41-0000": "Sales and Related",
    "43-0000": "Office and Administrative Support",
    "45-0000": "Farming, Fishing, and Forestry",
    "47-0000": "Construction and Extraction",
    "49-0000": "Installation, Maintenance, and Repair",
    "51-0000": "Production",
    "53-0000": "Transportation and Material Moving",
}


def generate_soc_groups_as_occupations() -> list[dict]:
    """Generate the 22 SOC major groups as placeholder occupations.

    Used when full BLS microdata isn't available — the discovery agent
    will still search for software per occupation group.
    """
    return [
        {
            "soc_code": code,
            "occupation_title": title,
            "soc_major_group": code[:2],
            "employment": 0,
            "mean_wage": 0,
            "wage_bill": 0,
            "gdp_total": 0,
        }
        for code, title in SOC_MAJOR_GROUPS.items()
    ]
