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
    assert resolve_checkpoint_paths(tmp_path / "run" / "model.safetensors") == (
        tmp_path / "run" / "model.safetensors",
        tmp_path / "run" / "config.json",
    )
    assert resolve_checkpoint_paths(tmp_path / "run" / "config.json") == (
        tmp_path / "run" / "model.safetensors",
        tmp_path / "run" / "config.json",
    )
    assert resolve_checkpoint_paths(tmp_path / "named.safetensors") == (
        tmp_path / "named.safetensors",
        tmp_path / "named.json",
    )

    # Standalone legacy pairs without canonical partners resolve to same-stem
    run_custom = tmp_path / "custom"
    run_custom.mkdir()
    (run_custom / "model.json").write_text("{}", encoding="utf-8")
    assert resolve_checkpoint_paths(run_custom / "model.safetensors") == (
        run_custom / "model.safetensors",
        run_custom / "model.json",
    )
    run_legacy_config = tmp_path / "legacy_config"
    run_legacy_config.mkdir()
    (run_legacy_config / "config.safetensors").write_text("", encoding="utf-8")
    assert resolve_checkpoint_paths(run_legacy_config / "config.json") == (
        run_legacy_config / "config.safetensors",
        run_legacy_config / "config.json",
    )

    # When canonical partner coexists with stale same-stem sibling, prefer canonical partner
    run_stale = tmp_path / "stale_siblings"
    run_stale.mkdir()
    (run_stale / "model.safetensors").write_text("", encoding="utf-8")
    (run_stale / "config.json").write_text("{}", encoding="utf-8")
    (run_stale / "model.json").write_text("{}", encoding="utf-8")
    (run_stale / "config.safetensors").write_text("", encoding="utf-8")
    assert resolve_checkpoint_paths(run_stale / "model.safetensors") == (
        run_stale / "model.safetensors",
        run_stale / "config.json",
    )
    assert resolve_checkpoint_paths(run_stale / "config.json") == (
        run_stale / "model.safetensors",
        run_stale / "config.json",
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
    loaded_state, config, metadata = load_checkpoint_files(tmp_path / "checkpoint")
    loaded_from_weights, _, _ = load_checkpoint_files(weights_path)
    loaded_from_config, _, _ = load_checkpoint_files(config_path)

    assert set(loaded_from_weights) == set(state)
    assert set(loaded_from_config) == set(state)
    assert weights_path.suffix == ".safetensors"
    assert config_path.suffix == ".json"
    assert config == {"encoder": "tiny", "width": 3}
    assert metadata == {"epoch": 2}
    assert set(loaded_state) == set(state)
    for name, tensor in state.items():
        assert torch.equal(loaded_state[name], tensor)


def test_safetensors_checkpoint_round_trip_ignores_stale_same_stem_siblings(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    checkpoint_dir = tmp_path / "checkpoint"
    checkpoint_dir.mkdir()

    # Create stale/outdated same-stem sibling files before saving
    (checkpoint_dir / "model.json").write_text('{"stale": true}', encoding="utf-8")
    (checkpoint_dir / "config.safetensors").write_bytes(b"stale_data")

    state = {
        "layer.weight": torch.tensor([[1.0, 2.0], [3.0, 4.0]]),
    }
    weights_path, config_path = save_checkpoint_files(
        checkpoint_dir,
        state,
        {"width": 2},
        {"version": 1},
    )

    loaded_dir, config_dir, _ = load_checkpoint_files(checkpoint_dir)
    loaded_weights, config_weights, _ = load_checkpoint_files(weights_path)
    loaded_config, config_from_cfg, _ = load_checkpoint_files(config_path)

    for s, c in [(loaded_dir, config_dir), (loaded_weights, config_weights), (loaded_config, config_from_cfg)]:
        assert c == {"width": 2}
        assert torch.equal(s["layer.weight"], state["layer.weight"])


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
