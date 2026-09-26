from __future__ import annotations

import json

import pytest

from cua_s1.checkpoint import (
    load_checkpoint_files,
    resolve_checkpoint_paths,
    save_checkpoint_files,
)


@pytest.mark.parametrize("suffix", [".pt", ".pth", ".bin", ".pkl", ".pickle"])
def test_pickle_based_checkpoints_are_rejected(suffix, tmp_path):
    with pytest.raises(ValueError, match="pickle-based"):
        resolve_checkpoint_paths(tmp_path / f"model{suffix}")


def test_checkpoint_path_resolution_is_explicit(tmp_path):
    assert resolve_checkpoint_paths(tmp_path / "run") == (
        tmp_path / "run" / "model.safetensors",
        tmp_path / "run" / "config.json",
    )
    assert resolve_checkpoint_paths(tmp_path / "named.safetensors") == (
        tmp_path / "named.safetensors",
        tmp_path / "named.json",
    )
    assert resolve_checkpoint_paths(tmp_path / "run" / "model.safetensors") == (
        tmp_path / "run" / "model.safetensors",
        tmp_path / "run" / "config.json",
    )
    assert resolve_checkpoint_paths(tmp_path / "run" / "config.json") == (
        tmp_path / "run" / "model.safetensors",
        tmp_path / "run" / "config.json",
    )


def test_safetensors_checkpoint_round_trip_uses_data_only_files(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    state = {
        "projection.weight": torch.arange(6, dtype=torch.float32).reshape(2, 3),
        "projection.bias": torch.tensor([0.25, -0.5]),
    }

    weights_path, config_path = save_checkpoint_files(
        tmp_path / "checkpoint",
        state,
        {"encoder": "tiny", "width": 3},
        {"epoch": 2},
    )
    assert weights_path == tmp_path / "checkpoint" / "model.safetensors"
    assert config_path == tmp_path / "checkpoint" / "config.json"

    for locator in (tmp_path / "checkpoint", weights_path, config_path):
        loaded_state, config, metadata = load_checkpoint_files(locator)
        assert config == {"encoder": "tiny", "width": 3}
        assert metadata == {"epoch": 2}
        assert set(loaded_state) == set(state)
        for name, tensor in state.items():
            assert torch.equal(loaded_state[name], tensor)


def test_checkpoint_rejects_malformed_or_wrong_format_json(tmp_path):
    pytest.importorskip("safetensors")
    weights_path, config_path = resolve_checkpoint_paths(tmp_path / "checkpoint")
    weights_path.parent.mkdir()
    weights_path.write_bytes(b"not safetensors")
    config_path.write_text("not json", encoding="utf-8")

    with pytest.raises(ValueError, match="invalid checkpoint JSON"):
        load_checkpoint_files(tmp_path / "checkpoint")

    config_path.write_text(
        json.dumps({"format": "other", "format_version": 1, "config": {}}),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="unsupported checkpoint format"):
        load_checkpoint_files(tmp_path / "checkpoint")


def test_checkpoint_requires_json_serializable_metadata(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")

    with pytest.raises(ValueError, match="JSON serializable"):
        save_checkpoint_files(
            tmp_path / "checkpoint",
            {"weight": torch.ones(1)},
            {"encoder": "tiny"},
            {"bad": object()},
        )


def test_checkpoint_rejects_mismatched_weight_and_config_pairs(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    first = tmp_path / "first"
    second = tmp_path / "second"
    save_checkpoint_files(first, {"weight": torch.ones(2)}, {"width": 2})
    save_checkpoint_files(second, {"weight": torch.zeros(3)}, {"width": 3})

    (first / "config.json").write_bytes((second / "config.json").read_bytes())

    with pytest.raises(ValueError, match="state signature mismatch"):
        load_checkpoint_files(first)


def test_checkpoint_rejects_config_tampering_even_when_shapes_match(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    checkpoint = tmp_path / "checkpoint"
    save_checkpoint_files(checkpoint, {"weight": torch.ones(2)}, {"width": 2})
    config_path = checkpoint / "config.json"
    document = json.loads(config_path.read_text(encoding="utf-8"))
    document["config"]["width"] = 9
    config_path.write_text(json.dumps(document), encoding="utf-8")

    with pytest.raises(ValueError, match="state signature mismatch"):
        load_checkpoint_files(checkpoint)


def test_named_checkpoint_pair_round_trips_through_returned_paths(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    state = {"weight": torch.tensor([1.0, 2.0])}

    weights_path, config_path = save_checkpoint_files(
        tmp_path / "named.safetensors",
        state,
        {"width": 2},
        {"source": "named"},
    )

    assert weights_path == tmp_path / "named.safetensors"
    assert config_path == tmp_path / "named.json"

    for locator in (weights_path, config_path):
        loaded_state, config, metadata = load_checkpoint_files(locator)
        assert config == {"width": 2}
        assert metadata == {"source": "named"}
        assert torch.equal(loaded_state["weight"], state["weight"])


def test_existing_ambiguous_same_stem_pairs_remain_compatible(tmp_path):
    model_weights = tmp_path / "model.safetensors"
    model_json = tmp_path / "model.json"
    model_json.write_text("{}", encoding="utf-8")
    assert resolve_checkpoint_paths(model_weights) == (model_weights, model_json)

    config_weights = tmp_path / "config.safetensors"
    config_json = tmp_path / "config.json"
    config_weights.write_bytes(b"placeholder")
    assert resolve_checkpoint_paths(config_json) == (config_weights, config_json)
