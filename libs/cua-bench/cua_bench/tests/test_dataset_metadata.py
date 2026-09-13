import tomllib
from pathlib import Path


DATASETS_DIR = Path(__file__).resolve().parents[2] / "datasets"


def test_all_dataset_pyprojects_are_valid_toml():
    invalid = []
    for pyproject in sorted(DATASETS_DIR.glob("*/*/pyproject.toml")):
        try:
            with pyproject.open("rb") as file:
                tomllib.load(file)
        except tomllib.TOMLDecodeError as error:
            invalid.append(f"{pyproject.relative_to(DATASETS_DIR)}: {error}")

    assert not invalid, "Invalid dataset metadata:\n" + "\n".join(invalid)
