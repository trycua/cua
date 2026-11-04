"""Deterministic synthetic training data for the Cua-S1 form specialist.

Each episode describes one form, one person, and document-style entities. Every
actionable element becomes a choice-classification row. Splits are assigned by
the form's concept signature, so the same field set cannot cross split
boundaries.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Sequence

from .concepts import (
    CHROME_ELEMENTS,
    CONCEPT_BY_KEY,
    CONCEPTS,
    DISTRACTOR_ENTITIES,
    FORM_TITLES,
    NON_SUBMIT_BUTTONS,
    OPTIONAL_CHECKBOXES,
    OPTIONAL_FIELDS,
    REQUIRED_CHECKBOXES,
    SUBMIT_LABELS,
    person,
)
from .schema import Element, Entity, encode, render_context, render_options

SPLIT_NAMES = ("train", "validation", "test")
DATASET_FORMAT = "cua-s1-choice-jsonl"
DATASET_FORMAT_VERSION = 1


@dataclass(frozen=True)
class SynthesisConfig:
    """Controls stochastic data features while preserving seeded replay."""

    missing_entity_probability: float = 0.12
    partial_form_probability: float = 0.30
    filled_field_probability: float = 0.60
    stale_value_probability: float = 0.05
    hard_negative_probability: float = 0.35
    max_extra_entities: int = 5
    max_distractor_entities: int = 3

    def validate(self) -> None:
        probabilities = (
            self.missing_entity_probability,
            self.partial_form_probability,
            self.filled_field_probability,
            self.stale_value_probability,
            self.hard_negative_probability,
        )
        if any(probability < 0.0 or probability > 1.0 for probability in probabilities):
            raise ValueError("synthesis probabilities must be between 0 and 1")
        if self.max_extra_entities < 0 or self.max_distractor_entities < 0:
            raise ValueError("entity limits must be non-negative")


def _stable_fraction(text: str) -> float:
    digest = hashlib.sha256(text.encode("utf-8")).digest()
    return int.from_bytes(digest[:8], "big") / 2**64


def _validate_ratios(ratios: Sequence[float]) -> tuple[float, float, float]:
    if len(ratios) != len(SPLIT_NAMES):
        raise ValueError("ratios must contain train, validation, and test values")
    normalized = tuple(float(ratio) for ratio in ratios)
    if any(ratio < 0 for ratio in normalized):
        raise ValueError("split ratios must be non-negative")
    total = sum(normalized)
    if total <= 0:
        raise ValueError("at least one split ratio must be positive")
    return tuple(ratio / total for ratio in normalized)  # type: ignore[return-value]


def sample_form(rng: random.Random, config: SynthesisConfig | None = None) -> dict:
    """Create a form with confusable labels, optional fields, and one submit."""

    config = config or SynthesisConfig()
    groups = sorted({concept.group for concept in CONCEPTS})
    chosen_groups = rng.sample(groups, rng.randint(2, min(5, len(groups))))
    pool = [concept for concept in CONCEPTS if concept.group in chosen_groups]
    concepts = rng.sample(pool, min(len(pool), rng.randint(4, 16)))

    # Co-locate look-alike concepts so matching requires the complete label.
    hard_negatives = (
        ("email", "street"),
        ("phone", "ec_phone"),
        ("full_name", "ec_name"),
        ("first_name", "ec_name"),
        ("phone", "work_phone"),
        ("dob", "start_date"),
        ("dob", "incident_date"),
        ("zip", "policy"),
        ("state", "university"),
        ("city", "country"),
        ("employer", "insurer"),
        ("account", "policy"),
    )
    for first, second in hard_negatives:
        if rng.random() < config.hard_negative_probability:
            for key in (first, second):
                if all(concept.key != key for concept in concepts):
                    concepts.append(CONCEPT_BY_KEY[key])

    keys = {concept.key for concept in concepts}
    if "full_name" in keys and {"first_name", "last_name"} & keys and rng.random() < 0.8:
        concepts = [concept for concept in concepts if concept.key != "full_name"]

    fields = [
        {
            "concept": concept.key,
            "label": rng.choice(concept.form_labels),
            "kind": concept.kind,
            "placeholder": (
                rng.choice(concept.placeholder)
                if concept.placeholder and rng.random() < 0.5
                else ""
            ),
        }
        for concept in concepts
    ]
    for _ in range(rng.choice((0, 0, 1, 1, 2))):
        fields.append(
            {
                "concept": None,
                "label": rng.choice(OPTIONAL_FIELDS),
                "kind": "text",
                "placeholder": "",
            }
        )
    rng.shuffle(fields)

    checkboxes = [(rng.choice(REQUIRED_CHECKBOXES), True) for _ in range(rng.choice((0, 1, 1, 2)))]
    checkboxes.extend(
        (rng.choice(OPTIONAL_CHECKBOXES), False) for _ in range(rng.choice((0, 0, 1)))
    )
    rng.shuffle(checkboxes)

    buttons = [(rng.choice(NON_SUBMIT_BUTTONS), False) for _ in range(rng.choice((0, 1, 1, 2)))]
    buttons.append((rng.choice(SUBMIT_LABELS), True))
    if rng.random() < 0.3:
        buttons.reverse()

    title = rng.choice(FORM_TITLES)
    if rng.random() < 0.5:
        title += rng.choice(
            (" - Google Chrome", " - Microsoft Edge", " - Mozilla Firefox", " - Safari", "")
        )
    if rng.random() < 0.2:
        title = rng.choice(("", "Untitled", "Form", "Page 2 of 3"))
    return {"title": title, "fields": fields, "checkboxes": checkboxes, "buttons": buttons}


def form_signature(form: dict) -> str:
    """Return the form identity used to keep train and evaluation disjoint."""

    fields = sorted(field["concept"] or f"optional:{field['label']}" for field in form["fields"])
    return "|".join(fields)


def sample_document(
    rng: random.Random,
    form: dict,
    profile: dict,
    config: SynthesisConfig | None = None,
) -> tuple[list[Entity], dict[str, int]]:
    """Generate document entities and a form-concept-to-entity index."""

    config = config or SynthesisConfig()
    entities: list[Entity] = []
    concept_to_entity: dict[str, int] = {}
    present = [field["concept"] for field in form["fields"] if field["concept"]]
    for key in present:
        if rng.random() < config.missing_entity_probability:
            continue
        concept = CONCEPT_BY_KEY[key]
        concept_to_entity[key] = len(entities)
        entities.append(Entity(rng.choice(concept.doc_labels), concept.value(rng, profile)))

    absent = [concept for concept in CONCEPTS if concept.key not in present]
    extra_count = rng.randint(0, min(config.max_extra_entities, len(absent)))
    for concept in rng.sample(absent, extra_count):
        entities.append(Entity(rng.choice(concept.doc_labels), concept.value(rng, profile)))

    distractor_count = rng.randint(0, min(config.max_distractor_entities, len(DISTRACTOR_ENTITIES)))
    for label, generate in rng.sample(DISTRACTOR_ENTITIES, distractor_count):
        entities.append(Entity(label, generate(rng)))

    order = list(range(len(entities)))
    rng.shuffle(order)
    entities = [entities[index] for index in order]
    remap = {old: new for new, old in enumerate(order)}
    concept_to_entity = {key: remap[index] for key, index in concept_to_entity.items()}
    return entities, concept_to_entity


def episode_rows(seed: int, config: SynthesisConfig | None = None) -> list[dict]:
    """Generate all element decisions for a single replayable episode."""

    config = config or SynthesisConfig()
    config.validate()
    rng = random.Random(seed)
    form = sample_form(rng, config)
    signature = form_signature(form)
    entities, concept_to_entity = sample_document(rng, form, person(rng), config)
    options = render_options(entities)
    partially_filled = rng.random() < config.partial_form_probability
    rows: list[dict] = []

    def add(
        element: Element,
        action: str,
        entity_index: int | None = None,
        placeholder: str = "",
    ) -> None:
        rows.append(
            {
                "context": render_context(form["title"], element, placeholder),
                "options": options,
                "label": encode(action, entity_index, entities),
                "meta": {
                    "seed": seed,
                    "form_signature": signature,
                    "role": element.role,
                    "action": action,
                },
            }
        )

    for field in form["fields"]:
        entity_index = concept_to_entity.get(field["concept"]) if field["concept"] else None
        filled = (
            entity_index is not None
            and partially_filled
            and rng.random() < config.filled_field_probability
        )
        value = entities[entity_index].value if filled and entity_index is not None else ""
        if filled and rng.random() < config.stale_value_probability:
            value = "stale value"
        element = Element("Edit", field["label"], value, actions=("set_value",))
        if entity_index is not None and (not value or value == "stale value"):
            add(element, "fill", entity_index, field["placeholder"])
        else:
            add(element, "skip", placeholder=field["placeholder"])

    for label, required in form["checkboxes"]:
        checked = partially_filled and rng.random() < 0.5
        element = Element("CheckBox", label, checked=checked, actions=("toggle",))
        add(element, "check" if required and not checked else "skip")

    for label, is_submit in form["buttons"]:
        add(Element("Button", label, actions=("invoke",)), "click" if is_submit else "skip")

    chrome_count = rng.randint(3, min(10, len(CHROME_ELEMENTS)))
    for role, label in rng.sample(CHROME_ELEMENTS, chrome_count):
        value = "https://example.invalid/form" if label == "Address bar" else ""
        add(Element(role, label or form["title"], value), "skip")
    return rows


def signature(rows: list[dict]) -> str:
    """Recover an episode signature from generated rows."""

    if not rows:
        raise ValueError("an episode must contain at least one row")
    return str(rows[0]["meta"]["form_signature"])


def write_splits(
    output: str | Path,
    episodes: int,
    seed: int,
    ratios: Sequence[float] = (0.8, 0.1, 0.1),
    config: SynthesisConfig | None = None,
) -> dict:
    """Write deterministic JSONL splits and return row and episode counts."""

    if episodes < 0:
        raise ValueError("episodes must be non-negative")
    config = config or SynthesisConfig()
    config.validate()
    normalized_ratios = _validate_ratios(ratios)
    train_ratio, validation_ratio, _ = normalized_ratios
    output_path = Path(output)
    output_path.mkdir(parents=True, exist_ok=True)
    handles = {
        name: (output_path / f"{name}.jsonl").open("w", encoding="utf-8") for name in SPLIT_NAMES
    }
    counts = {name: 0 for name in SPLIT_NAMES}
    episodes_per_split = {name: 0 for name in SPLIT_NAMES}
    try:
        for episode_index in range(episodes):
            rows = episode_rows(seed + episode_index * 7919, config)
            bucket = _stable_fraction(signature(rows))
            if bucket < train_ratio:
                split = "train"
            elif bucket < train_ratio + validation_ratio:
                split = "validation"
            else:
                split = "test"
            for row in rows:
                handles[split].write(
                    json.dumps(row, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
                    + "\n"
                )
            counts[split] += len(rows)
            episodes_per_split[split] += 1
    finally:
        for handle in handles.values():
            handle.close()

    split_manifest = {}
    for name in SPLIT_NAMES:
        split_path = output_path / f"{name}.jsonl"
        split_manifest[name] = {
            "file": split_path.name,
            "sha256": hashlib.sha256(split_path.read_bytes()).hexdigest(),
            "rows": counts[name],
            "episodes": episodes_per_split[name],
        }
    manifest = {
        "format": DATASET_FORMAT,
        "format_version": DATASET_FORMAT_VERSION,
        "generator": {
            "format": "cua_s1.synth",
            "config": asdict(config),
        },
        "ratios": dict(zip(SPLIT_NAMES, normalized_ratios, strict=True)),
        "seed": seed,
        "episodes": episodes,
        "splits": split_manifest,
    }
    manifest_path = output_path / "manifest.json"
    temporary = output_path / ".manifest.json.tmp"
    temporary.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(manifest_path)
    return {"rows": counts, "episodes": episodes_per_split}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("data/cua-s1"))
    parser.add_argument("--episodes", type=int, default=6000)
    parser.add_argument("--seed", type=int, default=2026)
    parser.add_argument("--train-ratio", type=float, default=0.8)
    parser.add_argument("--validation-ratio", type=float, default=0.1)
    parser.add_argument("--test-ratio", type=float, default=0.1)
    parser.add_argument("--missing-entity-probability", type=float, default=0.12)
    parser.add_argument("--partial-form-probability", type=float, default=0.30)
    parser.add_argument("--filled-field-probability", type=float, default=0.60)
    parser.add_argument("--stale-value-probability", type=float, default=0.05)
    parser.add_argument("--hard-negative-probability", type=float, default=0.35)
    parser.add_argument("--max-extra-entities", type=int, default=5)
    parser.add_argument("--max-distractor-entities", type=int, default=3)
    args = parser.parse_args()
    config = SynthesisConfig(
        missing_entity_probability=args.missing_entity_probability,
        partial_form_probability=args.partial_form_probability,
        filled_field_probability=args.filled_field_probability,
        stale_value_probability=args.stale_value_probability,
        hard_negative_probability=args.hard_negative_probability,
        max_extra_entities=args.max_extra_entities,
        max_distractor_entities=args.max_distractor_entities,
    )
    result = write_splits(
        args.output,
        args.episodes,
        args.seed,
        (args.train_ratio, args.validation_ratio, args.test_ratio),
        config,
    )
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
